# Development and operations guide

Detailed build, installation, API and SSO notes. See the [README](../README.md) for an overview.

**Status: early development.** This repository starts with a functioning proxy and management foundation. It is not yet a production-ready replacement for Nginx Proxy Manager or Traefik. Docker support comes after bare-metal deployment.

## What works

- Exact-host HTTP routing and segment-aware longest-path matching.
- HTTP/HTTPS upstreams, with verified upstream TLS.
- Several upstreams per route with round-robin or least-connections balancing, passive and optional active health checks, automatic DNS refresh, and retries on another upstream.
- Per-route timeouts, concurrency and request-body limits.
- HTTP/1.1 and HTTP/2 (TLS and cleartext) from clients, WebSocket, streaming responses, and gRPC through HTTP/2 upstreams.
- Automatic HTTPS: a certificate per domain chosen by SNI, issued and renewed over ACME (HTTP-01, and DNS-01 for wildcards through a hook), swapped in without a restart, with an optional HTTP to HTTPS redirect and expiry and errors in the dashboard.
- A fallback certificate from files for unknown names.
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

Open **http://127.0.0.1:8081** and enter the token from `data/admin.token`. Initialization never overwrites an existing token. Do not commit or share this file. The proxy listens on port **8080** by default.

Start an example upstream in a separate terminal:

```sh
python3 -m http.server 9000 --bind 127.0.0.1
```

In the dashboard create a host with domain `demo.local`, path `/`, and upstream `http://127.0.0.1:9000`. Test without changing DNS:

```sh
curl -H 'Host: demo.local' http://127.0.0.1:8080/
```

For HTTPS, uncomment the `[https]` section in `passage.toml`; `[acme]` then orders certificates automatically (see below). `general.dns_refresh_secs` (default 30) sets how often upstream host names are looked up again.

For dashboard work, keep `passage serve` running and start the Vite dev server with hot reload; it forwards `/api` to `127.0.0.1:8081`:

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
  certificates/      mod.rs (service), model.rs (records, domains, renewal timing), sni.rs (per-name
                     selection), acme.rs (orders, DNS hook), manager.rs (background issuing),
                     material.rs (keys, CSRs), challenges.rs (HTTP-01 proofs)
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
  smoke.py           end-to-end scenarios (HTTP/1.1, failover, health, limits, WebSocket, h2 via curl, HTTPS)
  fake_acme.py       a small ACME authority with its own CA, for the HTTPS scenarios
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
ssh -L 8081:127.0.0.1:8081 user@server
```

Open http://127.0.0.1:8081 locally. Read the initial token on the server with `sudo cat /var/lib/passage/admin.token`. Keep the administration listener private; do not configure a public proxy route pointing to it.

To upgrade, stop the service, back up the data directory, rerun the installer with the new binary, and start it again. The installer preserves existing configuration and credentials. For a consistent backup, stop Passage before copying its complete data directory (including any SQLite WAL files). Automated backup/restore tooling is planned. The database migrates itself when a newer Passage opens it, and an older Passage refuses a database written by a newer one, so back up the data directory before upgrading.

## API

Management endpoints require `Authorization: Bearer <administrator-token>`:

| Method | Endpoint | Purpose |
|---|---|---|
| GET | `/healthz` | Local liveness; no token needed |
| GET | `/api/status` | Version and process counters |
| GET | `/api/upstreams` | Live backends per route: address, health, requests in flight |
| GET | `/api/certificates` | List certificates with status, expiry and last error |
| POST | `/api/certificates` | Order a certificate (`domains`, `challenge`: `http-01` or `dns-01`) |
| POST | `/api/certificates/{id}/renew` | Order a new certificate now |
| DELETE | `/api/certificates/{id}` | Delete a certificate (400 while an enabled HTTPS host relies on it) |
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
  "https": { "enabled": true, "redirect": true },
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

A hostname/path pair must be unique. Paths are preserved, not rewritten; a trailing slash in `path_prefix` is ignored, and prefixes may not contain dot segments or percent-encoding. Invalid input returns 400 with an `error` message (422 for malformed JSON or unknown fields), unknown IDs 404. Upstream host names are resolved when a route is saved and refreshed every `general.dns_refresh_secs`; every resolved address is a backend. Explicit HTTP/HTTPS origins only; no URL credentials, wildcard hosts, or upstream base paths. See [Upstream pools and resilience](architecture.md#upstream-pools-and-resilience) for exact behavior.

## HTTPS and certificates

```toml
[admin]                         # dashboard and API, loopback only
listen = "127.0.0.1:8081"

[http]
listen = "0.0.0.0:80"

[https]                         # leave out to disable HTTPS
listen = "0.0.0.0:443"
# Optional fallback certificate for unknown names and clients without SNI:
# certificate = "/etc/passage/tls/fullchain.pem"
# private_key = "/etc/passage/tls/privkey.pem"

[acme]
email = "admin@example.com"
terms_agreed = true          # accepts the authority's terms of service
# directory_url = "https://acme-v02.api.letsencrypt.org/directory"   # use the staging URL to experiment
# ca_certificate = "/etc/passage/acme-ca.pem"    # trusted root of a private ACME server
# dns_hook = "/etc/passage/dns-hook"             # enables DNS-01 and wildcard certificates
# dns_propagation_secs = 30
# renew_before_days = 30
```

In the dashboard, tick **Serve over HTTPS** on a host: its certificate is requested with HTTP-01 and shown under **Certificates** with its expiry and any error. The domain must resolve to this server and port 80 must reach Passage. Tick **Redirect HTTP to HTTPS** to send plain requests to the HTTPS address. For a wildcard, add a certificate for `*.example.com` (and `example.com`) with DNS-01; hosts it covers then reuse it without further orders.

The DNS hook is run directly, not through a shell, with three arguments:

```sh
/etc/passage/dns-hook present _acme-challenge.example.com <value>   # publish this TXT record
/etc/passage/dns-hook cleanup _acme-challenge.example.com <value>   # remove it again
```

It must exit 0 once the record is published (Passage then waits `dns_propagation_secs`), and may be invoked twice for the same name with different values when a certificate covers both `example.com` and `*.example.com`. Make it executable, owned by root, and not writable by the service user.

Try it without a public domain by pointing `directory_url` at a private ACME server and `ca_certificate` at its root; `tests/fake_acme.py` is such a server.

Certificate private keys and the ACME account key are stored unencrypted in `passage.sqlite`; they are protected only by the `0700` data directory, so treat backups of it as secrets.

A certificate cannot be deleted while an enabled host with HTTPS relies on it; turn HTTPS off on that host first. A host with HTTPS enabled and no covering certificate gets one ordered automatically.

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

The smoke test starts real upstream, auth, and Passage processes on ephemeral ports. It checks proxying, route updates without restarts, invalid-update rollback, path-confusion handling on protected prefixes, auth denial, spoofed identity stripping, TLS upstream verification, static inbound TLS, persistence after restart, load balancing, failover and recovery, health checks, timeouts, limits, WebSocket, streaming, HTTP/2 (needs `curl`), a load burst with an upstream crash, and automatic HTTPS against a fake ACME authority (HTTP-01 and DNS-01 issuance, per-name certificates, redirect, renewal under traffic, reported failures; needs the `openssl` command). `cargo test` also runs `tests/protocols.rs`, which covers HTTP/2 and gRPC trailers through a real Passage process. [Validation](validation.md) lists what each check proves and what remains untested.

## Roadmap

1. **Bare-metal foundation:** proxy, dashboard, persistence, forward auth, systemd.
2. **Certificates:** certificate status alerts, import of existing certificates, built-in DNS providers.
3. **Identity:** administrator accounts, dashboard OIDC, sessions, RBAC and groups.
4. **Operations:** rate limiting, weighted balancing and sticky sessions, configuration history/restore, backups, `.deb` packages and signed releases.
5. **Automation:** declarative configuration/import, Docker packaging and optional discovery.

See [architecture](architecture.md) for boundaries and implementation decisions. Passage is a working name; name/trademark availability has not been established.
