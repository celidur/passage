# Initial validation

Validated on Linux x86-64 with Rust 1.99.1 and Node.js 22-compatible tooling.

- Frontend TypeScript check and Vite production build passed.
- Rust formatting, compilation, and unit tests passed.
- Real-process smoke checks passed: token enforcement, embedded dashboard, live HTTP proxying, preserved path/query, exact-host matching, stripped spoofed identity, duplicate/invalid route rollback, unknown route IDs, encoded/duplicated/dot-segment path handling for protected prefixes, enable/disable, forward-auth denial and verified identity propagation, redirect handling, auth outage denial, inbound TLS, rejection of untrusted upstream certificates, restart persistence, and live deletion.
- Installer shell syntax passed. The installer and systemd service were not exercised on a real Debian/Ubuntu host.
- Dashboard login, listing, validation errors and route creation were checked in a browser. Provider-specific SSO integration was not performed.

## Compatibility

Pingora 0.9.0 initializes network streams with zero-capacity Tokio buffers. Newer Tokio releases (confirmed with 1.54.1) reject that and panic on the first accepted connection. Tokio is therefore pinned to `=1.48.0` in `Cargo.toml`, which passes the proxy smoke checks. Revisit this pin only alongside a Pingora upgrade and a full real-process proxy test.

Native OIDC, ACME, per-host SNI certificates, multi-upstream balancing and Docker are not implemented in this milestone.
