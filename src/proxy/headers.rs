//! Helpers for reading the downstream request and writing short responses.

use bytes::Bytes;
use pingora::{http::ResponseHeader, prelude::*};

/// Client-supplied identity or forwarding metadata is never trusted.
pub fn is_untrusted(name: &str) -> bool {
    name.starts_with("x-auth-")
        || name.starts_with("x-authentik-")
        || name.starts_with("remote-")
        || name.starts_with("x-forwarded-")
        || name == "forwarded"
}

pub fn strip_untrusted(session: &mut Session) {
    let untrusted: Vec<_> = session
        .req_header()
        .headers
        .keys()
        .filter(|name| is_untrusted(name.as_str()))
        .cloned()
        .collect();
    for name in untrusted {
        session.req_header_mut().remove_header(&name);
    }
}

/// Host header, falling back to the URI authority of absolute-form requests.
pub fn request_host(session: &Session) -> &str {
    let req = session.req_header();
    req.headers
        .get(http::header::HOST)
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri.authority().map(|a| a.as_str()))
        .unwrap_or_default()
}

pub fn content_length(session: &Session) -> Option<u64> {
    session
        .req_header()
        .headers
        .get(http::header::CONTENT_LENGTH)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

pub fn scheme(session: &Session) -> &'static str {
    let tls = session
        .as_downstream()
        .digest()
        .is_some_and(|d| d.ssl_digest.is_some());
    if tls { "https" } else { "http" }
}

pub fn client_ip(session: &Session) -> Option<String> {
    session
        .client_addr()
        .and_then(|a| a.as_inet())
        .map(|a| a.ip().to_string())
}

/// Writes an empty, uncacheable response and reports the request as handled.
pub async fn respond(session: &mut Session, status: u16, location: Option<&[u8]>) -> Result<bool> {
    let mut header = ResponseHeader::build(status, None)?;
    header.insert_header("content-length", "0")?;
    header.insert_header("cache-control", "no-store")?;
    if let Some(location) = location {
        header.insert_header("location", location)?;
    }
    session
        .write_response_header(Box::new(header), true)
        .await?;
    Ok(true)
}

/// Writes a short plain-text response and reports the request as handled.
pub async fn respond_text(session: &mut Session, status: u16, body: &str) -> Result<bool> {
    let mut header = ResponseHeader::build(status, None)?;
    header.insert_header("content-type", "text/plain")?;
    header.insert_header("content-length", body.len().to_string())?;
    header.insert_header("cache-control", "no-store")?;
    session
        .write_response_header(Box::new(header), false)
        .await?;
    session
        .write_response_body(Some(Bytes::copy_from_slice(body.as_bytes())), true)
        .await?;
    Ok(true)
}

/// The HTTPS URL for a request, for redirecting plain HTTP. The port is left out when it is 443.
pub fn https_location(authority: &str, path_and_query: &str, port: u16) -> Option<String> {
    let host = authority
        .parse::<http::uri::Authority>()
        .ok()?
        .host()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let port = if port == 443 {
        String::new()
    } else {
        format!(":{port}")
    };
    Some(format!("https://{host}{port}{path_and_query}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_targets_keep_path_and_query() {
        let location = |authority, path, port| https_location(authority, path, port);
        assert_eq!(
            location("App.Example.com:80", "/a/b?x=1", 443).as_deref(),
            Some("https://app.example.com/a/b?x=1")
        );
        assert_eq!(
            location("app.example.com.", "/", 8443).as_deref(),
            Some("https://app.example.com:8443/")
        );
        assert!(location("bad host", "/", 443).is_none());
    }

    #[test]
    fn identity_and_forwarding_headers_are_untrusted() {
        for name in [
            "x-auth-request-user",
            "x-authentik-username",
            "remote-user",
            "x-forwarded-for",
            "forwarded",
        ] {
            assert!(is_untrusted(name), "{name}");
        }
        for name in ["cookie", "authorization", "x-request-id"] {
            assert!(!is_untrusted(name), "{name}");
        }
    }
}
