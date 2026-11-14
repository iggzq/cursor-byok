#!/bin/sh
# Installs the overview collector binary as a systemd service on a Linux host.
#
# Run as root on the central server after uploading the binary:
#   scp target/x86_64-unknown-linux-gnu/release/cursor-overview-collector root@HOST:/root/
#   ssh root@HOST 'sh /root/install-collector.sh /root/cursor-overview-collector YOUR_API_KEY'
set -eu

BINARY="${1:-/root/cursor-overview-collector}"
API_KEY="${2:-}"
INSTALL_DIR=/opt/cursor-overview-collector
DATA_DIR=/var/lib/cursor-collector

if [ -z "$API_KEY" ]; then
    echo "usage: $0 <binary-path> <api-key>" >&2
    exit 1
fi
if [ ! -f "$BINARY" ]; then
    echo "binary not found: $BINARY" >&2
    exit 1
fi

if ! getent group collector >/dev/null 2>&1; then
    groupadd --system collector
fi
if ! id -u collector >/dev/null 2>&1; then
    useradd --system --no-create-home --shell /usr/sbin/nologin --gid collector collector
fi
COLLECTOR_GROUP="$(id -gn collector)"

install -d -m 755 "$INSTALL_DIR"
install -d -m 700 -o collector -g "$COLLECTOR_GROUP" "$DATA_DIR"
install -m 755 "$BINARY" "$INSTALL_DIR/cursor-overview-collector"

cat > /etc/systemd/system/cursor-overview-collector.service <<EOF
[Unit]
Description=Cursor BYOK overview aggregation collector
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=collector
Group=$COLLECTOR_GROUP
WorkingDirectory=$DATA_DIR
Environment=CURSOR_OVERVIEW_COLLECTOR_API_KEY=$API_KEY
Environment=CURSOR_OVERVIEW_COLLECTOR_ADDR=0.0.0.0:4399
Environment=CURSOR_DATABASE_URL=sqlite://$DATA_DIR/overview-collector.db
Environment=RUST_LOG=cursor_server=info
ExecStart=$INSTALL_DIR/cursor-overview-collector
Restart=always
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=$DATA_DIR
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable --now cursor-overview-collector
sleep 2
systemctl --no-pager --lines=15 status cursor-overview-collector || true
echo
echo "health check:"
curl --fail --silent -o /dev/null -w '  /healthz -> %{http_code}\n' http://127.0.0.1:4399/healthz || echo "  collector is not answering yet; check journalctl -u cursor-overview-collector"
echo
echo "Open TCP 4399 in the cloud security group, then set the desktop to:"
echo "  http://<this-host-public-ip>:4399"