# Validation

What has been tested, how, and what has not. Re-run everything with the commands in the [development guide](development.md#validation).

## Environments

- **Foundation milestone:** Linux x86-64, Rust 1.99.1, Node.js 22-compatible tooling.
- **Proxy reliability milestone:** macOS arm64, Rust 1.99.0, Python 3.14, curl 8.7.1 (nghttp2). CI runs the same commands on Ubuntu 24.04; its result for these milestones is not part of this document.
- **Automatic HTTPS milestone:** the same macOS setup, with OpenSSL 4.0.3 as the `openssl` command used by the fake ACME authority.

## Automated checks

| Check | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --all-targets` | clean |
| `cargo test` | 48 unit tests and 4 protocol tests pass |
| `python3 tests/smoke.py` | 29 real-process scenarios pass, two consecutive runs |
| Frontend `npm run build` (TypeScript check and Vite build) | passes |
| `sh -n scripts/install.sh` | passes |

**Unit tests** cover route and policy validation, path canonicalization, the routing table and pool reuse across configuration changes, round-robin and least-connections selection, setting a backend aside and its cooldown, trying every backend when none is healthy, DNS refresh against a fake resolver (new address, kept state, failed lookup keeps previous addresses, strict versus lenient startup), the concurrency limit, active-check thresholds, and the schema migration runner (fresh database, upgrade, newer database refused, rollback of a failing migration). The HTTPS code adds tests for configuration rules (listeners, fallback certificate and `[acme]`), domain normalization and wildcard coverage, renewal timing and retry backoff, certificate status, per-name certificate selection (exact over wildcard over fallback, longest-lived wins, atomic replacement), signing requests and chain parsing with OpenSSL, HTTP-01 proof storage, certificate persistence, and the redirect URL.

**Smoke scenarios** start real Passage, upstream and fake-auth processes on ephemeral ports. Besides the foundation checks (token enforcement, embedded dashboard, path and header handling, forward authentication, inbound TLS, untrusted upstream certificates, rollback of invalid changes, persistence across restart), they verify:

- Round-robin splits 20 requests 10/10 across two upstreams; least-connections uses both.
- With one upstream dead, 10 of 10 requests succeed, the dead address is reported unhealthy, and it rejoins after it comes back.
- With every upstream dead, requests get 502 within three seconds, and traffic resumes as soon as one returns.
- `localhost` upstreams work whether it resolves to `::1`, `127.0.0.1` or both.
- An HTTP active check removes an upstream failing `/health` and restores it; a TCP check flags a dead upstream before any request reaches it.
- A backend that drops connections is bypassed for GET, while a POST is not replayed.
- An upstream slower than `read_secs` yields 504 on time; `read_secs` also cuts silent streams and WebSockets, and a larger value keeps them.
- `max_connections` sheds the excess with 503 and capacity returns afterwards; oversize bodies get 413, declared or chunked.
- WebSocket echo (including a 70 kB message) and slot release on close; a response is relayed as produced (first part in under a second of a 1.5 s response).
- HTTP/2 clients over TLS (ALPN) and in cleartext (prior knowledge), and HTTP/1.1 unchanged.
- A burst of 400 requests from 32 threads all succeed and both upstreams carry a fair share; killing one upstream during a second burst costs at most 5% of the requests, and the next 50 all succeed.

**HTTPS scenarios** run Passage with `[acme]` pointed at `tests/fake_acme.py`, a small ACME authority over HTTPS with its own CA. It implements the RFC 8555 flow (accounts, orders, authorizations, challenges, finalization, download) and validates for real: HTTP-01 by fetching the proof from Passage's plain listener, DNS-01 by reading the records the DNS hook script published. Passage's real ACME client (`instant-acme`) talks to it.

- Enabling HTTPS on two hosts orders one certificate each over HTTP-01; each name is then served its own certificate over TLS (verified against the CA, with the right names), an unknown name gets the fallback certificate, and a restart serves them again without ordering anything.
- With redirect on, plain HTTP gets a 308 to the HTTPS address with path and query, other hosts stay on HTTP, and the CA can still fetch its proof over HTTP from a host that redirects everything else.
- Renewing a certificate swaps it in for the next handshake while a thread keeps making HTTPS requests: every one succeeds, and the served serial number changes.
- A wildcard and apex certificate is issued over DNS-01 through the hook, which removes its records afterwards; a host it covers needs no new order; the same domains cannot be ordered twice, and a wildcard over HTTP-01 is refused.
- When the CA rejects an order, the certificate shows `failed` with the CA's reason, the failure count and a retry time; a manual renewal then succeeds and clears the error.
- Routes that redirect without HTTPS, single-label hosts, and malformed certificate requests are rejected, and a rejected route orders nothing.

**Real authority.** Passage was run once against Let's Encrypt staging for a real domain forwarded from ports 80 and 443 to 8080 and 8443. The first attempts failed with the authority's own error (`Timeout during connect`) shown under the certificate while the network path was broken. Once it was fixed, the HTTP-01 order completed: the certificate was issued by the staging CA for the host, served for that SNI name over TLS with HTTP/2, and answered requests through the proxy. A manual check, not part of the automated suite.

**Protocol tests** (`cargo test --test protocols`) run an in-process HTTP/2 server that frames messages like gRPC, behind a real Passage process: a streamed response arrives message by message (not buffered) with `grpc-status` in the trailers and `te: trailers` preserved, a Trailers-Only error passes through, a stalled upstream gives 504 and a dead one 502, and an HTTP/2 client reaches an HTTP/1.1 upstream.

## Not validated

- Real DNS changes. Refresh logic is tested with a fake resolver; the system resolver and `/etc/hosts` were not changed under a running Passage.
- Connect and TLS-handshake timeouts against an address that silently drops packets, and the retry that follows them.
- Sustained load. The largest test is a 400-request burst; there are no throughput or latency measurements, no soak test, and no memory profile.
- Real gRPC libraries (tonic, grpc-go), bidirectional streaming with flow control, HTTP/2 to an HTTPS upstream through ALPN, WebSocket over TLS or HTTP/2, and request-body limits on HTTP/2 uploads.
- The installer and systemd service on a real Debian/Ubuntu host.
- Provider-specific SSO integration.
- The dashboard was checked in a browser on a throwaway instance wired to the fake authority: host list with live upstream health and HTTPS state, the edit dialog with its advanced and HTTPS settings, saving a host with HTTPS and redirect (which ordered a certificate), and the Certificates page with valid, issuing and failed certificates and the CA's error. Other browsers and narrow screens were not checked.
- Production Let's Encrypt and Pebble. HTTP-01 issuance was verified once against Let's Encrypt staging (see above), but production rate limits, DNS-01 against a real authority, and a real DNS provider behind the hook are untested.
- Automatic renewal on schedule. Renewal timing is unit-tested and renewal on demand is tested end to end, but no certificate was left to age into its renewal window.
- Handshakes for a name with no certificate and no fallback, wildcard certificates in a browser, and the redirect behind a port forward.
- Certificate behavior after the database is restored from a backup, and systemd hardening with a DNS hook that needs network or filesystem access.

## Compatibility

Pingora 0.9.0 initializes network streams with zero-capacity Tokio buffers. Newer Tokio releases (confirmed with 1.54.1) reject that and panic on the first accepted connection. Tokio is therefore pinned to `=1.48.0` in `Cargo.toml`, which passes the proxy checks. Revisit this pin only alongside a Pingora upgrade and a full real-process proxy test.

Pingora's `add_tls` does not offer HTTP/2 through ALPN, and cleartext HTTP/2 is off by default. Passage builds the TLS settings itself, enables `h2`, and turns on h2c for the plain listener.

Native OIDC, built-in DNS providers, OCSP stapling, weighted balancing, sticky sessions, rate limiting and Docker are not implemented.
