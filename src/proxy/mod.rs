//! The Pingora request pipeline: route lookup, forward-auth, then proxying.

mod forward_auth;
mod headers;

use crate::{
    routes::{CompiledRoute, canonical_path},
    state::AppState,
};
use async_trait::async_trait;
use forward_auth::{ForwardAuth, Identity};
use headers::{client_ip, request_host, respond, scheme, strip_untrusted};
use pingora::{http::RequestHeader, prelude::*};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(60);

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
}

impl RequestContext {
    fn route(&self) -> &CompiledRoute {
        self.route
            .as_deref()
            .expect("route selected in request_filter")
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

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let route = ctx.route();
        let mut peer = HttpPeer::new(route.address, route.tls, route.sni.clone());
        peer.options.connection_timeout = Some(CONNECT_TIMEOUT);
        peer.options.read_timeout = Some(IO_TIMEOUT);
        peer.options.write_timeout = Some(IO_TIMEOUT);
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        request.insert_header("host", &ctx.route().upstream_host)?;
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
            duration_ms = ctx.start.elapsed().as_millis() as u64,
            "access"
        );
    }
}
