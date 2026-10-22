//! The Pingora request pipeline: route lookup, admission, forward-auth, then proxying.

mod forward_auth;
mod headers;

use crate::{
    routes::{CompiledRoute, UpstreamProtocol, canonical_path},
    state::AppState,
    upstream::{Lease, Slot},
};
use async_trait::async_trait;
use bytes::Bytes;
use forward_auth::{ForwardAuth, Identity};
use headers::{client_ip, content_length, request_host, respond, scheme, strip_untrusted};
use pingora::{
    ErrorSource, ErrorType,
    http::{RequestHeader, ResponseHeader},
    prelude::*,
    protocols::{ALPN, TcpKeepalive},
    proxy::FailToProxy,
};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

pub struct PassageProxy {
    state: Arc<AppState>,
    auth: ForwardAuth,
}

impl PassageProxy {
    pub fn new(state: Arc<AppState>) -> anyhow::Result<Self> {
        Ok(Self {
            state,
            auth: ForwardAuth::new()?,
        })
    }
}

pub struct RequestContext {
    route: Option<Arc<CompiledRoute>>,
    identity: Identity,
    start: Instant,
    /// Held until the request ends, so the route concurrency limit sees it.
    slot: Option<Slot>,
    /// The backend serving this attempt; held for least-connections accounting.
    lease: Option<Lease>,
    /// Backends that refused the connection during this request.
    tried: Vec<SocketAddr>,
    uploaded: u64,
}

impl RequestContext {
    fn route(&self) -> &Arc<CompiledRoute> {
        self.route
            .as_ref()
            .expect("route selected in request_filter")
    }
}

/// Dead peers are detected within about a minute even on silent long-lived streams.
fn keepalive() -> TcpKeepalive {
    TcpKeepalive {
        idle: Duration::from_secs(30),
        interval: Duration::from_secs(10),
        count: 3,
        #[cfg(target_os = "linux")]
        user_timeout: Duration::ZERO,
    }
}

/// Slow upstreams surface as 504 and unreachable ones as 502, never as a hung request.
fn status_for(error: &Error) -> u16 {
    use ErrorType::*;
    match error.etype() {
        HTTPStatus(code) => *code,
        ConnectTimedout | TLSHandshakeTimedout | ReadTimedout | WriteTimedout
            if *error.esource() == ErrorSource::Upstream =>
        {
            504
        }
        _ => match error.esource() {
            ErrorSource::Upstream => 502,
            ErrorSource::Downstream => match error.etype() {
                WriteError | ReadError | ConnectionClosed => 0,
                _ => 400,
            },
            ErrorSource::Internal | ErrorSource::Unset => 500,
        },
    }
}

#[async_trait]
impl ProxyHttp for PassageProxy {
    type CTX = RequestContext;

    fn new_ctx(&self) -> RequestContext {
        RequestContext {
            route: None,
            identity: Vec::new(),
            start: Instant::now(),
            slot: None,
            lease: None,
            tried: Vec::new(),
            uploaded: 0,
        }
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        self.state.metrics.record_request();
        let Some(path) = canonical_path(session.req_header().uri.path()) else {
            return respond(session, 400, None).await;
        };
        // Look up in one statement so the snapshot guard is not held across forward-auth.
        let route = self
            .state
            .snapshot
            .load()
            .find(request_host(session), &path);
        let Some(route) = route else {
            return respond(session, 404, None).await;
        };
        strip_untrusted(session);
        ctx.route = Some(route.clone());

        // Shed load before spending an authentication round trip on the request.
        let Some(slot) = route.pool.admit(route.limits.max_connections) else {
            return respond(session, 503, None).await;
        };
        ctx.slot = Some(slot);
        let too_large = route
            .limits
            .max_request_body_bytes
            .zip(content_length(session))
            .is_some_and(|(max, length)| length > max);
        if too_large {
            return respond(session, 413, None).await;
        }

        if let Some(url) = &route.forward_auth_url {
            let verification = self.auth.prepare(session, url);
            match verification.send(&route.id).await {
                Ok(identity) => ctx.identity = identity,
                Err(denied) => {
                    return respond(session, denied.status, denied.location.as_deref()).await;
                }
            }
        }
        Ok(false)
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        _end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        // Chunked and HTTP/2 uploads have no Content-Length to check up front.
        let Some(chunk) = body else { return Ok(()) };
        ctx.uploaded += chunk.len() as u64;
        match ctx.route().limits.max_request_body_bytes {
            Some(max) if ctx.uploaded > max => {
                Error::e_explain(ErrorType::HTTPStatus(413), "request body too large")
            }
            _ => Ok(()),
        }
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let route = ctx.route().clone();
        let Some(lease) = route.pool.lease(&ctx.tried) else {
            return Error::e_explain(ErrorType::HTTPStatus(503), "no upstream available");
        };
        let backend = lease.backend().clone();
        ctx.lease = Some(lease);

        let origin = &backend.origin;
        let mut peer = HttpPeer::new(backend.address, origin.tls, origin.sni.clone());
        let options = &mut peer.options;
        options.connection_timeout = Some(route.timeouts.connect());
        options.total_connection_timeout = Some(route.timeouts.connect());
        options.read_timeout = Some(route.timeouts.read());
        options.write_timeout = Some(route.timeouts.write());
        options.tcp_keepalive = Some(keepalive());
        options.alpn = match route.protocol {
            UpstreamProtocol::Http1 => ALPN::H1,
            UpstreamProtocol::Auto if origin.tls => ALPN::H2H1,
            UpstreamProtocol::Auto => ALPN::H1,
            UpstreamProtocol::Http2 => ALPN::H2,
        };
        Ok(Box::new(peer))
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut Self::CTX,
        mut error: Box<Error>,
    ) -> Box<Error> {
        let (Some(route), Some(lease)) = (&ctx.route, &ctx.lease) else {
            return error;
        };
        let backend = lease.backend();
        route.pool.report_connect_failure(backend);
        ctx.tried.push(backend.address);
        // No byte has been sent yet, so any method may move to another backend.
        let retry =
            ctx.tried.len() <= route.retries as usize && route.pool.has_candidate(&ctx.tried);
        error.set_retry(retry);
        error
    }

    fn error_while_proxy(
        &self,
        peer: &HttpPeer,
        session: &mut Session,
        error: Box<Error>,
        ctx: &mut Self::CTX,
        client_reused: bool,
    ) -> Box<Error> {
        let mut error = error.more_context(format!("Peer: {peer}"));
        let no_response_yet =
            session.response_written().is_none() && *error.esource() != ErrorSource::Downstream;
        let timed_out = matches!(
            error.etype(),
            ErrorType::ReadTimedout | ErrorType::WriteTimedout
        );
        // A reused connection may merely have gone stale; a fresh one dropping the request
        // means the backend itself failed.
        let died = !client_reused
            && matches!(
                error.etype(),
                ErrorType::ConnectionClosed | ErrorType::ReadError | ErrorType::WriteError
            );
        if let Some(route) = &ctx.route
            && let Some(lease) = &ctx.lease
            && no_response_yet
            && (timed_out || died)
        {
            // A backend that accepts connections but never answers counts against its health.
            route.pool.report_connect_failure(lease.backend());
            if died {
                ctx.tried.push(lease.backend().address);
            }
        }
        if !session.req_header().method.is_idempotent() || session.as_ref().retry_buffer_truncated()
        {
            error.set_retry(false);
        } else if no_response_yet
            && died
            && let Some(route) = &ctx.route
        {
            // Like a refused connection, but the request may have been seen: idempotent only.
            let retry =
                ctx.tried.len() <= route.retries as usize && route.pool.has_candidate(&ctx.tried);
            error.set_retry(retry);
        } else {
            error.retry.decide_reuse(client_reused);
        }
        error
    }

    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
        _response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        if let (Some(route), Some(lease)) = (&ctx.route, &ctx.lease) {
            route.pool.report_response(lease.backend());
        }
        Ok(())
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        let origin = &ctx
            .lease
            .as_ref()
            .expect("backend selected in upstream_peer")
            .backend()
            .origin;
        request.insert_header("host", &origin.host_header)?;
        let host = request_host(session);
        if !host.is_empty() {
            request.insert_header("x-forwarded-host", host)?;
        }
        request.insert_header("x-forwarded-proto", scheme(session))?;
        if let Some(ip) = client_ip(session) {
            request.insert_header("x-forwarded-for", ip)?;
        }
        for (name, value) in &ctx.identity {
            request.insert_header(*name, value.as_str())?;
        }
        Ok(())
    }

    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        error: &Error,
        _ctx: &mut Self::CTX,
    ) -> FailToProxy {
        let code = status_for(error);
        if code > 0 {
            session.respond_error(code).await.unwrap_or_else(|error| {
                tracing::error!(%error, "failed to send error response to downstream");
            });
        }
        FailToProxy {
            error_code: code,
            can_reuse_downstream: false,
        }
    }

    async fn logging(&self, session: &mut Session, error: Option<&Error>, ctx: &mut Self::CTX) {
        let status = session.response_written().map_or(0, |r| r.status.as_u16());
        if error.is_some() || status >= 500 {
            self.state.metrics.record_failure();
        }
        // Queries, credentials and cookies are deliberately excluded from access logs.
        tracing::info!(
            route_id = ctx.route.as_ref().map_or("unmatched", |r| r.id.as_str()),
            method = %session.req_header().method,
            status,
            upstream = ctx.lease.as_ref().map(|l| l.backend().address.to_string()),
            attempts = ctx.tried.len() + usize::from(ctx.lease.is_some()),
            duration_ms = ctx.start.elapsed().as_millis() as u64,
            "access"
        );
    }
}
