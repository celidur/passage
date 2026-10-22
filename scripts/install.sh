#!/bin/sh
# Install a locally built binary. Does not download code or start the service.
set -eu
if [ "$(id -u)" -ne 0 ]; then
    echo "Run as root: sudo ./scripts/install.sh [path-to-passage-binary]" >&2
    exit 1
fi
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
project_dir=$(dirname -- "$script_dir")
binary=${1:-"$project_dir/target/release/passage"}
[ -f "$binary" ] || { echo "Build first: cargo build --release" >&2; exit 1; }
command -v systemctl >/dev/null || { echo "systemd is required" >&2; exit 1; }
if ! getent passwd passage >/dev/null; then
    useradd --system --user-group --home-dir /var/lib/passage --shell /usr/sbin/nologin passage
fi
install -d -m 0750 -o root -g passage /etc/passage
install -d -m 0700 -o passage -g passage /var/lib/passage
install -m 0755 "$binary" /usr/local/bin/passage
if [ ! -f /etc/passage/config.toml ]; then
    install -m 0640 -o root -g passage "$project_dir/packaging/config.toml" /etc/passage/config.toml
fi
install -m 0644 "$project_dir/packaging/passage.service" /etc/systemd/system/passage.service
if [ ! -f /var/lib/passage/admin.token ]; then
    runuser -u passage -- /usr/local/bin/passage --config /etc/passage/config.toml init
fi
systemctl daemon-reload
printf '%s\n' 'Installed. Existing configuration and data were preserved.' \
    'Start: sudo systemctl enable --now passage' \
    'Dashboard: ssh -L 8080:127.0.0.1:8080 user@server, then open http://127.0.0.1:8080' \
    'Token: sudo cat /var/lib/passage/admin.token'
