# Architecture

Passage is a Linux service containing an embedded React dashboard, an Axum control API, SQLite, and a Pingora 0.9 proxy. Node.js is a build-time dependency only. OpenSSL is a runtime system dependency. No Docker daemon is required.

## Request processing

The proxy selects an exact normalized Host and the longest segment-aware path prefix. Before matching, the request path is percent-decoded and repeated slashes are collapsed, so `//admin` or `/%61dmin` match the same route the upstream will serve. Paths containing `.`/`..` segments (encoded or not) or backslashes are rejected with 400, since they could resolve upstream to a route other than the one matched, for example bypassing forward authentication. Each request captures its selected route, so an in-flight request keeps a consistent configuration during updates. Requests preserve their original path and query; the upstream receives its own origin Host. Forwarded host, scheme, and client IP are regenerated rather than trusted from the client. Unknown hosts return 404.

Configuration writes are serialized. A candidate route set is validated and DNS-resolved (5-second timeout per upstream) before the single changed row is committed in one SQLite transaction, along with an audit entry. Only after a successful commit does ArcSwap publish the immutable snapshot. Invalid changes return 400 and leave both stored and live configuration intact; storage failures return 500 without exposing details. DNS resolves on save and startup, not per request. Periodic DNS refresh is a future milestone; DNS changes currently require re-saving the affected route or restarting. Multiple upstream addresses and balancing are not implemented yet.

The proxy does not consult SQLite or Axum for public routes during request handling. A stalled management API does not block proxy reads. Everything runs in one process; a process crash affects both planes.

## Administration

The management listener is restricted to loopback. Use an SSH tunnel for remote access. Initialization generates a random 256-bit administrator bearer token in a file with mode 0600. The token is not printed into logs. The API compares SHA-256 digests in constant time. The dashboard retains the token only in React memory, never in localStorage, URLs, or cookies. Reloading the page locks the dashboard. This is an initial single-administrator credential, not a complete user/role system.

An attacker with root access, service-account access, or the admin token can reconfigure upstreams; admin credentials are security-critical. The API intentionally accepts private-network upstreams because that is a reverse proxy's purpose. No browser CORS access is enabled. Bearer-only mutations avoid ambient-cookie CSRF. Administration is not safe to expose by adding a proxy route to its loopback port; do not do so.

## Forward authentication

An optional per-route `forward_auth_url` is called with a GET, the original Authorization and Cookie headers, and regenerated X-Forwarded-Method/Proto/Host/URI/For metadata. The HTTP client verifies TLS, refuses redirects, and times out after three seconds. 2xx permits the request. A redirect with Location is returned to the browser. 401 returns 401; other rejection statuses return 403; network failures return 503. No denied request reaches the upstream.

Only a fixed allowlist of Remote-* and X-Auth-Request-* / X-Forwarded-* identity response headers reaches the upstream. Client-supplied X-Auth-*, X-Authentik-*, Remote-*, X-Forwarded-*, and Forwarded headers are removed before authorization. Provider-specific response cookies and arbitrary identity headers are not supported in this initial implementation; configure the identity provider accordingly. Native OIDC for the dashboard, group policies, and interactive login flows are planned separately.

## Certificates and deployment

An optional static certificate/key enables an additional inbound TLS listener; upstream HTTPS validates certificates with OpenSSL. There is one certificate per listener, no per-host SNI certificate registry yet. Certificate replacement requires restart. ACME, renewal, DNS-01 and HTTP-01 are planned, not implemented.

The installer creates a dedicated system user, preserves existing config/data, and installs a hardened systemd unit. It deliberately leaves service activation to the operator. The service can bind low ports via CAP_NET_BIND_SERVICE rather than root. Releases should target Debian/Ubuntu x86-64 first. Docker, Kubernetes, clustering, and automatic discovery are later phases.
