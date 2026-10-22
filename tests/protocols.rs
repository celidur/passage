//! HTTP/2 and gRPC through a real Passage process.
//!
//! gRPC needs HTTP/2 end to end, with trailers, which the Python smoke tests cannot speak.
//! The upstream below frames messages like a gRPC server; no gRPC runtime is involved.

use bytes::{BufMut, Bytes, BytesMut};
use h2::{RecvStream, client, server};
use http::{HeaderMap, Request, Response};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener as StdListener},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use tokio::net::{TcpListener, TcpStream};

fn local_addr() -> SocketAddr {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

struct Passage {
    child: Child,
    proxy: SocketAddr,
    admin: SocketAddr,
    token: String,
    dir: tempfile::TempDir,
}

impl Passage {
    async fn start() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (proxy, admin) = (local_addr(), local_addr());
        let config = dir.path().join("passage.toml");
        let data = dir.path().join("data");
        std::fs::write(
            &config,
            format!(
                "[general]\ndata_dir = \"{}\"\n[admin]\nlisten = \"{admin}\"\n[http]\nlisten = \"{proxy}\"\n",
                data.display()
            ),
        )
        .unwrap();
        let binary = env!("CARGO_BIN_EXE_passage");
        let init = Command::new(binary)
            .args(["-c".as_ref(), config.as_os_str(), "init".as_ref()])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(init.success());
        let token = std::fs::read_to_string(data.join("admin.token"))
            .unwrap()
            .trim()
            .to_string();
        let log = std::fs::File::create(dir.path().join("server.log")).unwrap();
        let child = Command::new(binary)
            .args(["-c".as_ref(), config.as_os_str(), "serve".as_ref()])
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let passage = Self {
            child,
            proxy,
            admin,
            token,
            dir,
        };
        passage.wait_until_ready().await;
        passage
    }

    async fn wait_until_ready(&self) {
        let client = reqwest::Client::new();
        for _ in 0..100 {
            if client
                .get(format!("http://{}/healthz", self.admin))
                .send()
                .await
                .is_ok()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("Passage did not become ready");
    }

    async fn create_route(&self, route: Value) {
        let response = reqwest::Client::new()
            .post(format!("http://{}/api/routes", self.admin))
            .bearer_auth(&self.token)
            .header("content-type", "application/json")
            .body(route.to_string())
            .send()
            .await
            .unwrap();
        let status = response.status();
        assert_eq!(status, 201, "{}", response.text().await.unwrap());
    }
}

impl Drop for Passage {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let log = std::fs::read_to_string(self.dir.path().join("server.log"));
            eprintln!("--- Passage log ---\n{}", log.unwrap_or_default());
        }
    }
}

fn route(host: &str, upstream: SocketAddr, policy: Value) -> Value {
    json!({
        "name": host,
        "host": host,
        "upstreams": [format!("http://{upstream}")],
        "policy": policy,
    })
}

fn grpc_frame(message: &[u8]) -> Bytes {
    let mut frame = BytesMut::with_capacity(5 + message.len());
    frame.put_u8(0);
    frame.put_u32(message.len() as u32);
    frame.put_slice(message);
    frame.freeze()
}

/// A cleartext HTTP/2 server speaking the gRPC wire format.
///
/// `/pkg.Svc/Method` streams two messages 400 ms apart and ends with `grpc-status: 0`;
/// `/pkg.Svc/Missing` answers Trailers-Only with status 12; `/pkg.Svc/Stall` never answers.
async fn serve_grpc(listener: TcpListener) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            return;
        };
        tokio::spawn(async move {
            let Ok(mut connection) = server::handshake(socket).await else {
                return;
            };
            while let Some(Ok((request, respond))) = connection.accept().await {
                tokio::spawn(answer(request, respond));
            }
        });
    }
}

async fn answer(request: Request<RecvStream>, mut respond: server::SendResponse<Bytes>) {
    let path = request.uri().path().to_string();
    let te = request
        .headers()
        .get("te")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut body = request.into_body();
    while let Some(Ok(chunk)) = body.data().await {
        let _ = body.flow_control().release_capacity(chunk.len());
    }
    let grpc = Response::builder()
        .status(200)
        .header("content-type", "application/grpc");
    match path.as_str() {
        "/pkg.Svc/Stall" => tokio::time::sleep(Duration::from_secs(30)).await,
        "/pkg.Svc/Missing" => {
            let response = grpc.header("grpc-status", "12").body(()).unwrap();
            respond.send_response(response, true).unwrap();
        }
        _ => {
            let mut send = respond
                .send_response(grpc.body(()).unwrap(), false)
                .unwrap();
            let first = format!("path={path} te={te}");
            send.send_data(grpc_frame(first.as_bytes()), false).unwrap();
            tokio::time::sleep(Duration::from_millis(400)).await;
            send.send_data(grpc_frame(b"done"), false).unwrap();
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", "0".parse().unwrap());
            send.send_trailers(trailers).unwrap();
        }
    }
}

struct Reply {
    status: u16,
    headers: HeaderMap,
    /// Each message with the time it was fully received.
    messages: Vec<(Duration, String)>,
    trailers: Option<HeaderMap>,
    elapsed: Duration,
}

/// A unary gRPC call over cleartext HTTP/2 with prior knowledge.
async fn call(proxy: SocketAddr, host: &str, path: &str) -> Reply {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (client, connection) = client::handshake(TcpStream::connect(proxy).await.unwrap())
            .await
            .unwrap();
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::builder()
            .method("POST")
            .uri(format!("http://{host}{path}"))
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .body(())
            .unwrap();
        let started = Instant::now();
        let (response, mut stream) = client
            .ready()
            .await
            .unwrap()
            .send_request(request, false)
            .unwrap();
        stream.send_data(grpc_frame(b"ping"), true).unwrap();
        let response = response.await.unwrap();
        let (parts, mut body) = response.into_parts();

        let mut buffer = BytesMut::new();
        let mut messages = Vec::new();
        while let Some(chunk) = body.data().await {
            let chunk = chunk.unwrap();
            body.flow_control().release_capacity(chunk.len()).unwrap();
            buffer.extend_from_slice(&chunk);
            while buffer.len() >= 5 {
                let length = u32::from_be_bytes(buffer[1..5].try_into().unwrap()) as usize;
                if buffer.len() < 5 + length {
                    break;
                }
                let frame = buffer.split_to(5 + length);
                let text = String::from_utf8(frame[5..].to_vec()).unwrap();
                messages.push((started.elapsed(), text));
            }
        }
        let trailers = body.trailers().await.unwrap();
        Reply {
            status: parts.status.as_u16(),
            headers: parts.headers,
            messages,
            trailers,
            elapsed: started.elapsed(),
        }
    })
    .await
    .expect("call did not finish")
}

#[tokio::test]
async fn grpc_streams_messages_and_forwards_trailers() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(serve_grpc(upstream));
    let passage = Passage::start().await;
    let policy = json!({"protocol": "http2", "health": {"check": {"kind": "tcp"}}});
    passage
        .create_route(route("grpc.local", upstream_addr, policy))
        .await;

    let reply = call(passage.proxy, "grpc.local", "/pkg.Svc/Method").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.headers["content-type"], "application/grpc");
    let texts: Vec<_> = reply
        .messages
        .iter()
        .map(|(_, text)| text.as_str())
        .collect();
    assert_eq!(texts, ["path=/pkg.Svc/Method te=trailers", "done"]);
    let gap = reply.messages[1].0 - reply.messages[0].0;
    assert!(
        gap >= Duration::from_millis(250),
        "response was buffered: {gap:?}"
    );
    assert_eq!(reply.trailers.expect("trailers")["grpc-status"], "0");
}

#[tokio::test]
async fn grpc_trailers_only_errors_pass_through() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(serve_grpc(upstream));
    let passage = Passage::start().await;
    let policy = json!({"protocol": "http2"});
    passage
        .create_route(route("grpc.local", upstream_addr, policy))
        .await;

    let reply = call(passage.proxy, "grpc.local", "/pkg.Svc/Missing").await;
    assert_eq!(reply.status, 200);
    assert_eq!(reply.headers["grpc-status"], "12");
    assert!(reply.messages.is_empty());
}

#[tokio::test]
async fn stalled_grpc_upstream_times_out_and_dead_one_is_unavailable() {
    let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_addr = upstream.local_addr().unwrap();
    tokio::spawn(serve_grpc(upstream));
    let passage = Passage::start().await;
    let policy = json!({"protocol": "http2", "timeouts": {"read_secs": 1}});
    passage
        .create_route(route("stall.local", upstream_addr, policy.clone()))
        .await;
    let dead = local_addr();
    passage
        .create_route(route("dead.local", dead, policy))
        .await;

    let reply = call(passage.proxy, "stall.local", "/pkg.Svc/Stall").await;
    assert_eq!(reply.status, 504, "gRPC clients map 504 to UNAVAILABLE");
    assert!(
        reply.elapsed < Duration::from_secs(4),
        "{:?}",
        reply.elapsed
    );

    let reply = call(passage.proxy, "dead.local", "/pkg.Svc/Method").await;
    assert_eq!(reply.status, 502);
}

/// Answers every request with `hello` over HTTP/1.1.
fn serve_http1() -> SocketAddr {
    let listener = StdListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
            });
        }
    });
    address
}

#[tokio::test]
async fn http2_client_reaches_an_http1_upstream() {
    let upstream = serve_http1();
    let passage = Passage::start().await;
    passage
        .create_route(route("h1.local", upstream, json!({})))
        .await;

    let (client, connection) = client::handshake(TcpStream::connect(passage.proxy).await.unwrap())
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = connection.await;
    });
    let request = Request::builder().uri("http://h1.local/").body(()).unwrap();
    let (response, _) = client
        .ready()
        .await
        .unwrap()
        .send_request(request, true)
        .unwrap();
    let response = response.await.unwrap();
    assert_eq!(response.status(), 200);
    let mut body = response.into_body();
    let mut received = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        body.flow_control().release_capacity(chunk.len()).unwrap();
        received.extend_from_slice(&chunk);
    }
    assert_eq!(received, b"hello");
}
