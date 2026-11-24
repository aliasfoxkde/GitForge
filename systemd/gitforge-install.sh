#!/bin/bash
# GitForge Service Installation Script
set -e

INSTALL_DIR="/nas/Temp/repos/GitForge"
SYSTEMD_DIR="/etc/systemd/system"
SERVICE_USER="gitforge"
SERVICE_GROUP="gitforge"
HOST_STORAGE_TMPFILES="/home/mkinney/repos/backend-fixed/systemd/tmpfiles.d/codex-host-storage.conf"
ENABLE_LOCAL_RUNNER="${GITFORGE_ENABLE_LOCAL_RUNNER:-0}"

if [[ "$ENABLE_LOCAL_RUNNER" != "0" && "$ENABLE_LOCAL_RUNNER" != "1" ]]; then
    echo "GITFORGE_ENABLE_LOCAL_RUNNER must be 0 or 1" >&2
    exit 2
fi

# Masking/disablement only controls future starts. It does not stop a runner
# that is already executing jobs, so never report the local executor retired
# while its process may still be claiming work. Drain through GitForge's
# supported control plane and verify terminal receipts before stopping it.
if [[ "$ENABLE_LOCAL_RUNNER" == "0" ]]; then
    RUNNER_STATE="$(systemctl is-active gitforge@runner.service 2>/dev/null || true)"
    case "$RUNNER_STATE" in
        active|activating|deactivating|reloading|maintenance)
            echo "Refusing to mask active GitForge runner ($RUNNER_STATE). Pause/drain it through the supported control plane, verify terminal job receipts and zero owned containers, stop it, then rerun this installer." >&2
            exit 1
            ;;
        inactive|failed|unknown)
            ;;
        *)
            echo "Unable to determine GitForge runner state (${RUNNER_STATE:-no output}); refusing to mask it." >&2
            exit 1
            ;;
    esac
fi

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
install -D -m 0644 \
    "$INSTALL_DIR/systemd/gitforge@runner.service.d/host-storage-lease.conf" \
    "$SYSTEMD_DIR/gitforge@runner.service.d/host-storage-lease.conf"
if [[ ! -f "$HOST_STORAGE_TMPFILES" ]]; then
    echo "Missing canonical host storage lease config: $HOST_STORAGE_TMPFILES" >&2
    exit 1
fi
install -m 0644 "$HOST_STORAGE_TMPFILES" /etc/tmpfiles.d/codex-host-storage.conf
systemd-tmpfiles --create /etc/tmpfiles.d/codex-host-storage.conf

# Enable and start services
echo "Enabling GitForge services..."
systemctl daemon-reload
systemctl enable gitforge.target

# The NAS installation is the control plane. Heavy execution belongs on
# remote runners (normally Fedora); a local executor is an explicit opt-in.
for svc in api git-server ci; do
    echo "Starting gitforge@$svc..."
    systemctl enable "gitforge@$svc"
    systemctl start "gitforge@$svc" || echo "Warning: Failed to start gitforge@$svc"
done

if [[ "$ENABLE_LOCAL_RUNNER" == "1" ]]; then
    echo "Enabling the explicitly requested local GitForge runner..."
    systemctl unmask gitforge@runner.service
    systemctl enable gitforge@runner.service
    systemctl start gitforge@runner.service || echo "Warning: Failed to start gitforge@runner"
else
    echo "Keeping the NAS-local runner retired; remote runners execute CI jobs."
    systemctl disable gitforge@runner.service 2>/dev/null || true
    systemctl mask gitforge@runner.service
fi

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
echo "  - gitforge@api        : REST API (port 42780)"
echo "  - gitforge@git-server : Git SSH (42022) / HTTP (42782)"
echo "  - gitforge@ci         : CI Orchestrator"
echo "  - gitforge@runner     : disabled on NAS by default (set GITFORGE_ENABLE_LOCAL_RUNNER=1 to opt in)"
echo ""
echo "Web UI: http://localhost:42780/swagger-ui"
