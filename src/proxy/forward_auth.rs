//! Delegates access decisions to an external SSO verification endpoint.

use super::headers::{client_ip, request_host, scheme};
use pingora::prelude::*;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);

/// Identity headers copied from a successful verification response to the upstream request.
const IDENTITY_HEADERS: &[&str] = &[
    "remote-user",
    "remote-email",
    "remote-groups",
    "remote-name",
    "x-auth-request-user",
    "x-auth-request-email",
    "x-auth-request-groups",
    "x-auth-request-preferred-username",
    "x-forwarded-user",
    "x-forwarded-email",
    "x-forwarded-groups",
    "x-forwarded-preferred-username",
];

pub type Identity = Vec<(&'static str, String)>;

/// The response returned to the client when access is refused.
pub struct Denied {
    pub status: u16,
    pub location: Option<Vec<u8>>,
}

pub struct ForwardAuth {
    client: reqwest::Client,
}

/// A verification request detached from the session, so it can be awaited without borrowing it.
pub struct Verification(reqwest::RequestBuilder);

impl ForwardAuth {
    pub fn new() -> anyhow::Result<Self> {
        // Redirects from the auth service are for the browser and must not be followed here.
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    pub fn prepare(&self, session: &Session, url: &str) -> Verification {
        let req = session.req_header();
        let uri = req.uri.path_and_query().map_or("/", |v| v.as_str());
        let mut request = self
            .client
            .get(url)
            .header("x-forwarded-method", req.method.as_str())
            .header("x-forwarded-proto", scheme(session))
            .header("x-forwarded-host", request_host(session))
            .header("x-forwarded-uri", uri);
        if let Some(ip) = client_ip(session) {
            request = request.header("x-forwarded-for", ip);
        }
        for name in [http::header::COOKIE, http::header::AUTHORIZATION] {
            if let Some(value) = req.headers.get(&name) {
                request = request.header(name, value.clone());
            }
        }
        Verification(request)
    }
}

impl Verification {
    /// 2xx allows; a redirect is returned to the browser; 401 stays 401; anything else is 403.
    /// An unreachable auth service fails closed with 503.
    pub async fn send(self, route_id: &str) -> Result<Identity, Denied> {
        let response = self.0.send().await.map_err(|error| {
            tracing::warn!(route_id, %error, "authentication service unavailable");
            Denied {
                status: 503,
                location: None,
            }
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(IDENTITY_HEADERS
                .iter()
                .filter_map(|&name| {
                    let value = response.headers().get(name)?.to_str().ok()?;
                    Some((name, value.to_owned()))
                })
                .collect());
        }
        let location = status
            .is_redirection()
            .then(|| response.headers().get(http::header::LOCATION))
            .flatten()
            .map(|v| v.as_bytes().to_vec());
        let status = match status.as_u16() {
            code if location.is_some() => code,
            401 => 401,
            _ => 403,
        };
        Err(Denied { status, location })
    }
}
