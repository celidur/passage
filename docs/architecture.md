# Architecture

Passage is a Linux service containing an embedded React dashboard, an Axum control API, SQLite, and a Pingora 0.9 proxy. Node.js is a build-time dependency only. OpenSSL is a runtime system dependency. No Docker daemon is required.

## Request processing

The proxy selects an exact normalized Host and the longest segment-aware path prefix. Before matching, the request path is percent-decoded and repeated slashes are collapsed, so `//admin` or `/%61dmin` match the same route the upstream will serve. Paths containing `.`/`..` segments (encoded or not) or backslashes are rejected with 400, since they could resolve upstream to a route other than the one matched, for example bypassing forward authentication. Each request captures its selected route, so an in-flight request keeps a consistent configuration during updates. Requests preserve their original path and query; the upstream receives its own origin Host. Forwarded host, scheme, and client IP are regenerated rather than trusted from the client. Unknown hosts return 404.

Configuration writes are serialized. A candidate route set is validated before the single changed row is committed in one SQLite transaction, along with an audit entry. Hostnames of new or changed upstreams must resolve (5-second timeout) or the change is rejected. Only after a successful commit does ArcSwap publish the immutable snapshot, and routes whose upstream settings did not change keep their pool, with its health and load state. Invalid changes return 400 (422 for malformed JSON) and leave both stored and live configuration intact; storage failures return 500 without exposing details.

The proxy does not consult SQLite or Axum for public routes during request handling. A stalled management API does not block proxy reads. Everything runs in one process; a process crash affects both planes.

## Upstream pools and resilience

Each enabled route owns a pool of one to sixteen upstream origins. **Every address an origin resolves to is a backend**, so a name with several A/AAAA records spreads traffic across all of them, and an unreachable `::1` next to a working `127.0.0.1` is just a backend that gets skipped.

**Selection.** `round_robin` rotates over the backends that are currently usable; `least_connections` picks the one with the fewest requests in flight. A backend counts as unusable while it is set aside by passive health or failing its active check. When *no* backend is usable, the pool is tried anyway instead of answering at once, so a wrong health verdict can never black-hole a route, and a recovered backend is found without waiting for a cooldown. Retries only go to usable backends that have not been tried in the same request.

**Passive health.** `health.max_failures` consecutive connection failures, timeouts before the first response byte, or connections dropped before any response set a backend aside for `health.cooldown_secs`. After the cooldown it is tried again, and its first response clears the counter. A response of any status, including 5xx, counts as alive: applications may legitimately answer 503.

**Active health.** `health.check` is optional. Every `interval_secs` each backend gets an HTTP GET on `path` (2xx and 3xx are healthy, redirects are not followed, HTTP/1.1 only, certificates verified like proxied traffic) or a plain TCP connection (`kind: tcp`, for HTTP/2-only upstreams such as gRPC). A backend changes state only after `unhealthy_threshold` or `healthy_threshold` consecutive results. Backends start healthy until their first probe.

**DNS refresh.** Hostnames are looked up again every `dns_refresh_secs` (default 30; the system resolver exposes no TTL) and every two seconds while a name has not resolved. Up to 16 addresses per name are kept, in sorted order so that a rotated answer is not a change. Backends whose address is unchanged keep their health and load. A failed or empty lookup keeps the last known addresses and logs a warning. Startup tolerates names that do not resolve yet, and the route starts answering 503 until they do; `passage check` and saves through the API are strict and refuse them.

**Retries.** A refused or timed-out connection never reached the application, so any method moves to another backend, up to `policy.retries` extra attempts. A backend that drops a fresh connection before answering is bypassed only for idempotent methods, so a POST is never replayed. Read timeouts are never retried: a slow application is not sent the same work twice.

**Limits and timeouts.** `timeouts.connect_secs` covers the TCP connect and TLS handshake; `read_secs` is the longest wait for the next data from the upstream, which also cuts streams, Server-Sent Events and WebSockets that stay silent that long, so raise it for those; `write_secs` bounds each write. `limits.max_connections` admits that many concurrent requests per route (a WebSocket or stream counts until it ends) and sheds the rest before any authentication call. `limits.max_request_body_bytes` rejects declared oversize bodies up front and uploads without a declared length (chunked) as they cross the limit. TCP keepalive probes detect dead peers on silent connections.

| Situation | Client receives |
|---|---|
| Connection refused or unreachable on every backend tried | 502 |
| Connect or TLS handshake exceeds the connect timeout | 504 |
| No data within the read timeout before the response starts | 504 |
| No address resolved yet for any upstream | 503 |
| Route at `max_connections` | 503 |
| Request body over `max_request_body_bytes` | 413 |

gRPC clients map 502, 503 and 504 to UNAVAILABLE.

**Protocols.** Clients may use HTTP/1.1 or HTTP/2, over TLS (ALPN) or in cleartext with prior knowledge (h2c). Upstream protocol is per route: `http1` (default), `auto` (HTTP/2 when an HTTPS upstream offers it) or `http2` (h2c on `http://`, ALPN `h2` on `https://`), which gRPC requires so that trailers survive. WebSocket upgrades are relayed, and responses are streamed as the upstream produces them rather than buffered.

## Administration

The management listener is restricted to loopback. Use an SSH tunnel for remote access. Initialization generates a random 256-bit administrator bearer token in a file with mode 0600. The token is not printed into logs. The API compares SHA-256 digests in constant time. The dashboard retains the token only in React memory, never in localStorage, URLs, or cookies. Reloading the page locks the dashboard. This is an initial single-administrator credential, not a complete user/role system.

An attacker with root access, service-account access, or the admin token can reconfigure upstreams; admin credentials are security-critical. The API intentionally accepts private-network upstreams because that is a reverse proxy's purpose. No browser CORS access is enabled. Bearer-only mutations avoid ambient-cookie CSRF. Administration is not safe to expose by adding a proxy route to its loopback port; do not do so.

## Forward authentication

An optional per-route `forward_auth_url` is called with a GET, the original Authorization and Cookie headers, and regenerated X-Forwarded-Method/Proto/Host/URI/For metadata. The HTTP client verifies TLS, refuses redirects, and times out after three seconds. 2xx permits the request. A redirect with Location is returned to the browser. 401 returns 401; other rejection statuses return 403; network failures return 503. No denied request reaches the upstream.

Only a fixed allowlist of Remote-* and X-Auth-Request-* / X-Forwarded-* identity response headers reaches the upstream. Client-supplied X-Auth-*, X-Authentik-*, Remote-*, X-Forwarded-*, and Forwarded headers are removed before authorization. Provider-specific response cookies and arbitrary identity headers are not supported in this initial implementation; configure the identity provider accordingly. Native OIDC for the dashboard, group policies, and interactive login flows are planned separately.

## Certificates and deployment

An optional static certificate/key enables an additional inbound TLS listener that negotiates HTTP/2 or HTTP/1.1; upstream HTTPS validates certificates with OpenSSL. There is one certificate per listener, no per-host SNI certificate registry yet. Certificate replacement requires restart. ACME, renewal, DNS-01 and HTTP-01 are planned, not implemented.

The installer creates a dedicated system user, preserves existing config/data, and installs a hardened systemd unit. It deliberately leaves service activation to the operator. The service can bind low ports via CAP_NET_BIND_SERVICE rather than root. Releases should target Debian/Ubuntu x86-64 first. Docker, Kubernetes, clustering, and automatic discovery are later phases.
