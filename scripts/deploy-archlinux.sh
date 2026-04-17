#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN_DIR="$HOME/.local/bin"
BIN_PATH="$BIN_DIR/ember"
SERVICE_DIR="$CONFIG_HOME/systemd/user"
SERVICE_FILE="$SERVICE_DIR/ember.service"
CONFIG_FILE="$CONFIG_HOME/ember/config.toml"
MAN_DIR="$HOME/.local/share/man/man1"

mkdir -p "$BIN_DIR" "$SERVICE_DIR" "$DATA_HOME/ember" "$CONFIG_HOME/ember" "$MAN_DIR"

echo "==> Building Ember (release)"
cargo build --release --manifest-path "$ROOT/Cargo.toml"

install -Dm755 "$ROOT/target/release/ember" "$BIN_PATH"
install -Dm644 "$ROOT/man/ember.1" "$MAN_DIR/ember.1"

if [[ ! -f "$CONFIG_FILE" ]]; then
  echo "==> Installing default config"
  install -Dm644 "$ROOT/config/default.toml" "$CONFIG_FILE"
else
  echo "==> Keeping existing config at $CONFIG_FILE"
fi

cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Ember notification daemon
After=graphical-session-pre.target
PartOf=graphical-session.target

[Service]
Type=simple
ExecStart=$BIN_PATH
Restart=on-failure
RestartSec=2
Environment=RUST_LOG=ember=info

[Install]
WantedBy=default.target
EOF

echo "==> Reloading user systemd"
systemctl --user daemon-reload
systemctl --user enable --now ember.service

echo
echo "Ember deployed successfully."
echo "Binary:  $BIN_PATH"
echo "Config:  $CONFIG_FILE"
echo "Service: $SERVICE_FILE"
echo
echo "Check status with: systemctl --user status ember.service"
