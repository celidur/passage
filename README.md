# Passage

**A clear path to your services.**

A simple, self-hosted reverse proxy manager powered by Rust and Cloudflare Pingora.

Passage aims to combine Nginx Proxy Manager’s approachable interface with Traefik-style automation, with easy bare-metal deployment as the first priority.

> **Status: early development.** The bare-metal foundation works and is tested end to end, but Passage is not yet a production-ready replacement for Nginx Proxy Manager or Traefik.

## Features

| | Feature |
|---|---|
| ✅ | Pingora proxy with exact-host and longest-path-prefix routing, HTTP/HTTPS upstreams with verified TLS |
| ✅ | Several upstreams per route with round-robin or least-connections balancing, health checks, automatic DNS refresh and failover |
| ✅ | Per-route timeouts, concurrency and request-size limits, so a slow or dead application stays contained |
| ✅ | HTTP/2, WebSocket, streaming responses and gRPC |
| ✅ | Web dashboard to create, edit, search, disable and delete routes |
| ✅ | SQLite persistence with an audit log; changes apply live, without restarting |
| ✅ | Automatic HTTPS: tick a box on a host and its certificate is ordered over ACME (Let’s Encrypt or another authority), chosen per domain by SNI, renewed and swapped in without dropping a connection |
| ✅ | HTTP-01 validation, DNS-01 for wildcard certificates through a DNS hook, HTTP to HTTPS redirect, and certificate expiry and errors in the dashboard |
| ✅ | Per-route forward authentication to protect applications behind an external SSO (provider-specific testing pending) |
| ✅ | Install script and hardened systemd service |
| 🔜 | Built-in DNS providers for wildcard certificates |
| 🔜 | Administrator accounts and dashboard login through an identity provider (OIDC) |
| 🔜 | Backups, rate limiting, `.deb` packages, Docker |

## Quick start

Requires Linux or macOS, stable Rust, a C compiler, OpenSSL headers, pkg-config and Node.js 22+.

```sh
(cd web && npm ci && npm run build)   # the dashboard is embedded in the binary
cargo build --locked
./target/debug/passage init           # creates ./data and the administrator token
./target/debug/passage serve
```

Open <http://127.0.0.1:8081> and paste the token from `data/admin.token`. The proxy listens on port 8080 with the default [passage.toml](passage.toml):

```sh
python3 -m http.server 9000 --bind 127.0.0.1 &   # an example upstream
# In the dashboard: domain demo.local, path /, upstream http://127.0.0.1:9000
curl -H 'Host: demo.local' http://127.0.0.1:8080/
```

To install on a Debian/Ubuntu server:

```sh
cargo build --release --locked
sudo ./scripts/install.sh ./target/release/passage
sudo systemctl enable --now passage
```

The administration interface only listens on loopback; reach it with `ssh -L 8081:127.0.0.1:8081 user@server`.

## Documentation

- [Development and operations guide](docs/development.md): build, install, upgrade, API, SSO setup, project layout.
- [Architecture](docs/architecture.md): request processing, security model, design decisions and limits.
- [Validation](docs/validation.md): what has been tested, and known compatibility constraints.

## Project layout

```text
src/        Rust: proxy/ (Pingora pipeline), upstream/ (backend pools, health, DNS refresh), certificates/
            (SNI, ACME, renewal), api/ (management API), routes/ (model, policy and routing table), state,
            store (SQLite and migrations), commands (init/check/serve)
web/src/    React dashboard: api/ client, layout/, components/, features/<name>/
tests/      End-to-end tests: smoke.py (+ harness.py, fake_acme.py) and protocols.rs (HTTP/2, gRPC)
packaging/  systemd unit and default server configuration
scripts/    install.sh
```

See [docs/development.md](docs/development.md#project-layout) for the detailed layout and where new features go.

## Development

```sh
cargo fmt --check && cargo test --locked && cargo build --locked
python3 tests/smoke.py target/debug/passage
cd web && npm run dev    # dashboard with hot reload, proxies /api to a running `passage serve`
```

CI runs the same checks on every push.

## Technology

Rust · Pingora · Axum · React / TypeScript · Vite · SQLite

## License

[Apache-2.0](LICENSE). Passage is a working name; name/trademark availability has not been established.
