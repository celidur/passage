# Passage

A simple, self-hosted reverse proxy manager powered by Rust and Cloudflare Pingora.

Passage aims to combine Nginx Proxy Manager’s approachable interface with Traefik-style automation, with easy bare-metal deployment as the first priority.

## Goals

- Manage domains, upstream services, and routing from a web dashboard.
- Automate HTTPS certificate issuance and renewal.
- Protect applications with SSO and integrate administrator login with an identity provider.
- Apply routing changes without restarting the proxy.
- Install as a single Linux service, with Docker support planned afterward.

## Technology

Rust · Pingora · Axum · React / TypeScript · SQLite

## Status

Early development. The initial implementation is being reviewed on a separate branch. Features listed above describe the project’s goals; they are not all available yet.
