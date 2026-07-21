#!/bin/bash
# GitForge Service Installation Script
set -e

INSTALL_DIR="/nas/Temp/repos/GitForge"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_USER="gitforge"
SERVICE_GROUP="gitforge"

echo "🔧 Installing GitForge services..."

# Create gitforge user if it doesn't exist
if ! id "$SERVICE_USER" &>/dev/null; then
    echo "Creating user: $SERVICE_USER"
    /usr/sbin/useradd --system --no-create-home --shell /usr/sbin/nologin $SERVICE_USER
fi

# Create data directory
mkdir -p "$INSTALL_DIR/data"
chown -R $SERVICE_USER:$SERVICE_GROUP "$INSTALL_DIR/data"

# Copy systemd units
echo "Installing systemd units..."
cp "$INSTALL_DIR/systemd/gitforge.target" "$SYSTEMD_DIR/"
cp "$INSTALL_DIR/systemd/gitforge@.service" "$SYSTEMD_DIR/"

# Enable and start services
echo "Enabling GitForge services..."
systemctl daemon-reload
systemctl enable gitforge.target

# Start individual services
for svc in api git-server ci runner; do
    echo "Starting gitforge@$svc..."
    systemctl enable "gitforge@$svc"
    systemctl start "gitforge@$svc" || echo "Warning: Failed to start gitforge@$svc"
done

echo ""
echo "✅ GitForge services installed!"
echo ""
echo "Usage:"
echo "  systemctl start gitforge.target   # Start all services"
echo "  systemctl stop gitforge.target    # Stop all services"
echo "  systemctl restart gitforge@api   # Restart specific service"
echo "  journalctl -u gitforge@api        # View API logs"
echo ""
echo "Services:"
echo "  - gitforge@api        : REST API (port 8080)"
echo "  - gitforge@git-server : Git SSH (2222) / HTTP (8082)"
echo "  - gitforge@ci         : CI Orchestrator"
echo "  - gitforge@runner     : Job Runner"
echo ""
echo "Web UI: http://localhost:8080/swagger-ui"