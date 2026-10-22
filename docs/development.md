# Development and operations guide

Detailed build, installation, API and SSO notes. See the [README](../README.md) for an overview.

**Status: early development.** This repository starts with a functioning proxy and management foundation. It is not yet a production-ready replacement for Nginx Proxy Manager or Traefik. Docker support comes after bare-metal deployment.

## What works

- Exact-host HTTP routing and segment-aware longest-path matching.
- HTTP/HTTPS upstreams, with verified upstream TLS.
- Several upstreams per route with round-robin or least-connections balancing, passive and optional active health checks, automatic DNS refresh, and retries on another upstream.
- Per-route timeouts, concurrency and request-body limits.
- HTTP/1.1 and HTTP/2 (TLS and cleartext) from clients, WebSocket, streaming responses, and gRPC through HTTP/2 upstreams.
- Optional inbound HTTPS using an existing certificate.
- Embedded React/TypeScript dashboard: create, edit, search, disable, and delete hosts.
- SQLite persistence and audit records for route mutations.
- Validated live configuration snapshots, without restarting for route changes.
- Per-route forward authentication for integration with an external SSO service.
- Loopback administration protected by a generated administrator token.
- Request/failure counters, structured journal logs, and health endpoint.
- Bare-metal installer and hardened systemd service.

## Build and run locally

Prerequisites: Linux (macOS also works for development), current stable Rust, C compiler, OpenSSL development headers, pkg-config, and Node.js 22+ with npm. Debian/Ubuntu build dependencies:

```sh
sudo apt-get install build-essential cmake pkg-config libssl-dev
cd web
npm ci
npm run build
cd ..
cargo build --locked
./target/debug/passage init
./target/debug/passage check
./target/debug/passage serve
```

Open **http://127.0.0.1:8080** and enter the token from `data/admin.token`. Initialization never overwrites an existing token. Do not commit or share this file. The proxy listens on port **8081** by default.

Start an example upstream in a separate terminal:

```sh
python3 -m http.server 9000 --bind 127.0.0.1
```

In the dashboard create a host with domain `demo.local`, path `/`, and upstream `http://127.0.0.1:9000`. Test without changing DNS:

```sh
curl -H 'Host: demo.local' http://127.0.0.1:8081/
```

For TLS, uncomment the `[tls]` section in `passage.toml` and set existing certificate/key paths. HTTPS termination and certificate automation are separate features; **automatic certificates are not implemented yet**. `dns_refresh_secs` (default 30) sets how often upstream host names are looked up again.

For dashboard work, keep `passage serve` running and start the Vite dev server with hot reload; it forwards `/api` to `127.0.0.1:8080`:

```sh
cd web && npm run dev
```

## Project layout

```text
src/
  main.rs            CLI parsing only
  lib.rs             module list
  commands.rs        init / check / serve
  config.rs          passage.toml
  state.rs           AppState shared by proxy and API; applies configuration changes
  store.rs           SQLite persistence, audit log and schema migrations
  token.rs           administrator token
  metrics.rs         request/failure counters
  routes/            model.rs (validation), policy.rs (balancing, timeouts, limits, health),
                     path.rs (canonical paths), snapshot.rs (routing table)
  upstream/          pool.rs (backends, selection, health, DNS refresh), check.rs (active probes),
                     resolver.rs, maintenance.rs (background ticker)
  proxy/             mod.rs (Pingora pipeline), forward_auth.rs, headers.rs
  api/               mod.rs (router, service), one file per resource, error.rs, json.rs, middleware.rs, assets.rs
web/src/
  main.tsx, App.tsx  entry point and session (login vs dashboard)
  api/               typed client and API types
  layout/            AppShell and Sidebar
  components/        shared UI (Dialog, alerts)
  hooks/, lib/       shared hooks and helpers
  features/<name>/   one folder per feature: page, components, data hook, CSS
  styles/base.css    reset, typography and shared primitives
tests/
  harness.py         process/HTTP helpers and fake upstream/auth services
  smoke.py           end-to-end scenarios (HTTP/1.1, failover, health, limits, WebSocket, h2 via curl)
  protocols.rs       HTTP/2 and gRPC (trailers) through a real Passage process
```

Adding a feature usually means a new `src/api/<resource>.rs` merged in `api::router`, domain code in its own module with unit tests next to it, a `web/src/features/<name>/` folder, and a scenario in `tests/smoke.py`.

## Install on a Debian/Ubuntu server

Build on a compatible Linux distribution. The resulting executable embeds the dashboard; Node.js is not needed on the destination server. OpenSSL and CA certificates must be installed there.

```sh
cargo build --release --locked
sudo ./scripts/install.sh ./target/release/passage
sudo systemctl enable --now passage
sudo journalctl -u passage -f
```

The service uses `/etc/passage/config.toml`, stores data in `/var/lib/passage`, and listens on port 80. Ensure it does not conflict with an existing web server. It runs as `passage`, with permission to bind low ports.

For remote administration:

```sh
ssh -L 8080:127.0.0.1:8080 user@server
```

Open http://127.0.0.1:8080 locally. Read the initial token on the server with `sudo cat /var/lib/passage/admin.token`. Keep the administration listener private; do not configure a public proxy route pointing to it.

To upgrade, stop the service, back up the data directory, rerun the installer with the new binary, and start it again. The installer preserves existing configuration and credentials. For a consistent backup, stop Passage before copying its complete data directory (including any SQLite WAL files). Automated backup/restore tooling is planned. The database migrates itself when a newer Passage opens it, and an older Passage refuses a database written by a newer one, so back up the data directory before upgrading.

## API

Management endpoints require `Authorization: Bearer <administrator-token>`:

| Method | Endpoint | Purpose |
|---|---|---|
| GET | `/healthz` | Local liveness; no token needed |
| GET | `/api/status` | Version and process counters |
| GET | `/api/upstreams` | Live backends per route: address, health, requests in flight |
| GET | `/api/routes` | List all hosts |
| POST | `/api/routes` | Create a host |
| PUT | `/api/routes/{id}` | Replace a host |
| DELETE | `/api/routes/{id}` | Remove a host |

Create/update body:

```json
{
  "name": "My app",
  "host": "app.example.com",
  "path_prefix": "/",
  "upstreams": ["http://10.0.0.5:3000", "http://app.internal:3000"],
  "enabled": true,
  "forward_auth_url": null,
  "policy": {
    "balancing": "least_connections",
    "timeouts": { "read_secs": 30 },
    "limits": { "max_connections": 200 },
    "health": { "check": { "path": "/healthz" } }
  }
}
```

`policy` and each of its parts are optional; omitted fields take the defaults below, and the API returns the complete policy.

| Setting | Default | Meaning |
|---|---|---|
| `balancing` | `round_robin` | or `least_connections` |
| `protocol` | `http1` | upstream protocol: `auto` (HTTP/2 over HTTPS when offered) or `http2` (required for gRPC) |
| `retries` | 2 (max 5) | extra attempts on other backends after a connection failure |
| `timeouts.connect_secs` | 5 | TCP connect and TLS handshake |
| `timeouts.read_secs` | 60 | longest wait for upstream data; also cuts silent streams and WebSockets |
| `timeouts.write_secs` | 60 | longest blocked write to the upstream |
| `limits.max_connections` | none | concurrent requests per route; excess get 503 |
| `limits.max_request_body_bytes` | none | larger request bodies get 413 |
| `health.max_failures` | 3 | consecutive failures before a backend is set aside |
| `health.cooldown_secs` | 10 | how long a failed backend is skipped |
| `health.check` | none | periodic probe: `kind` (`http` or `tcp`), `path` (`/`), `interval_secs` (10), `timeout_secs` (3), `unhealthy_threshold` (2), `healthy_threshold` (2) |

A hostname/path pair must be unique. Paths are preserved, not rewritten; a trailing slash in `path_prefix` is ignored, and prefixes may not contain dot segments or percent-encoding. Invalid input returns 400 with an `error` message (422 for malformed JSON or unknown fields), unknown IDs 404. Upstream host names are resolved when a route is saved and refreshed every `dns_refresh_secs`; every resolved address is a backend. Explicit HTTP/HTTPS origins only; no URL credentials, wildcard hosts, or upstream base paths. See [Upstream pools and resilience](architecture.md#upstream-pools-and-resilience) for exact behavior.

## SSO integration

Set `forward_auth_url` to the verification endpoint of your external authentication service. Passage forwards the original Cookie/Authorization plus regenerated forwarding metadata. 2xx allows access; denial or service failure prevents upstream access. Redirects go back to the browser instead of being followed internally. Identity headers are copied only from an allowlist. Provider-specific integration testing is still needed before relying on a particular Authentik/Authelia deployment. See [authentication behavior and limitations](architecture.md).

This protects application access at the proxy. Logging into an application's own account additionally requires that application's support for OIDC or trusted proxy identity. Dashboard OIDC is a future milestone.

## Validation

```sh
cargo fmt --check
cargo test --locked
cargo build --locked
python3 tests/smoke.py target/debug/passage
sh -n scripts/install.sh
```

The smoke test starts real upstream, auth, and Passage processes on ephemeral ports. It checks proxying, route updates without restarts, invalid-update rollback, path-confusion handling on protected prefixes, auth denial, spoofed identity stripping, TLS upstream verification, static inbound TLS, persistence after restart, load balancing, failover and recovery, health checks, timeouts, limits, WebSocket, streaming, HTTP/2 (needs `curl`) and a load burst with an upstream crash. `cargo test` also runs `tests/protocols.rs`, which covers HTTP/2 and gRPC trailers through a real Passage process. [Validation](validation.md) lists what each check proves and what remains untested.

## Roadmap

1. **Bare-metal foundation:** proxy, dashboard, persistence, forward auth, systemd.
2. **Certificates:** per-host SNI, ACME issuance and renewal, certificate status.
3. **Identity:** administrator accounts, dashboard OIDC, sessions, RBAC and groups.
4. **Operations:** rate limiting, weighted balancing and sticky sessions, configuration history/restore, backups, `.deb` packages and signed releases.
5. **Automation:** declarative configuration/import, Docker packaging and optional discovery.

See [architecture](architecture.md) for boundaries and implementation decisions. Passage is a working name; name/trademark availability has not been established.
