# Development and operations guide

Detailed build, installation, API and SSO notes. See the [README](../README.md) for an overview.

**Status: early development.** This repository starts with a functioning proxy and management foundation. It is not yet a production-ready replacement for Nginx Proxy Manager or Traefik. Docker support comes after bare-metal deployment.

## What works

- Exact-host HTTP routing and segment-aware longest-path matching.
- HTTP/HTTPS upstreams, with verified upstream TLS.
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

For TLS, uncomment the `[tls]` section in `passage.toml` and set existing certificate/key paths. HTTPS termination and certificate automation are separate features; **automatic certificates are not implemented yet**.

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
  store.rs           SQLite persistence and audit log
  token.rs           administrator token
  metrics.rs         request/failure counters
  routes/            model.rs (validation), path.rs (canonical paths), snapshot.rs (routing table)
  proxy/             mod.rs (Pingora pipeline), forward_auth.rs, headers.rs
  api/               mod.rs (router, service), one file per resource, error.rs, middleware.rs, assets.rs
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
  smoke.py           end-to-end scenarios
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

To upgrade, stop the service, back up the data directory, rerun the installer with the new binary, and start it again. The installer preserves existing configuration and credentials. For a consistent backup, stop Passage before copying its complete data directory (including any SQLite WAL files). Automated backup/restore tooling and schema migrations beyond the initial schema are planned.

## API

Management endpoints require `Authorization: Bearer <administrator-token>`:

| Method | Endpoint | Purpose |
|---|---|---|
| GET | `/healthz` | Local liveness; no token needed |
| GET | `/api/status` | Version and process counters |
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
  "upstream": "http://127.0.0.1:3000",
  "enabled": true,
  "forward_auth_url": null
}
```

A hostname/path pair must be unique. Paths are preserved, not rewritten; a trailing slash in `path_prefix` is ignored, and prefixes may not contain dot segments or percent-encoding. Invalid input returns 400 with an `error` message, unknown IDs 404. Upstream DNS resolves on startup/save; periodic refresh is not implemented. Explicit HTTP/HTTPS origins only; no URL credentials, wildcard hosts, or upstream base paths.

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

The smoke test starts real upstream, auth, and Passage processes on ephemeral ports. It checks proxying, route updates without restarts, invalid-update rollback, path-confusion handling on protected prefixes, auth denial, spoofed identity stripping, TLS upstream verification, static inbound TLS, and persistence after restart.

## Roadmap

1. **Bare-metal foundation:** proxy, dashboard, persistence, forward auth, systemd.
2. **Certificates:** per-host SNI, ACME issuance and renewal, certificate status.
3. **Identity:** administrator accounts, dashboard OIDC, sessions, RBAC and groups.
4. **Operations:** upstream health checks, multiple backends, DNS refresh, rate limiting, configuration history/restore, backups, `.deb` packages and signed releases.
5. **Automation:** declarative configuration/import, Docker packaging and optional discovery.

See [architecture](architecture.md) for boundaries and implementation decisions. Passage is a working name; name/trademark availability has not been established.
