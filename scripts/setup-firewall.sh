#!/bin/bash
# GitForge Firewall Setup Script
# Run with sudo: sudo ./scripts/setup-firewall.sh

set -e

# New GitForge Ports
API_PORT=42780
SCHEDULER_PORT=42781
GIT_HTTP_PORT=42782
GIT_SSH_PORT=42022

echo "🔧 GitForge Firewall Configuration"
echo "=================================="

# Check if running as root
if [ "$EUID" -ne 0 ]; then
    echo "Please run as root (sudo)"
    exit 1
fi

echo "Adding firewall rules for GitForge ports..."

# Remove old GitForge rules if they exist
echo "Removing old rules..."
iptables -D INPUT -p tcp --dport 8080 -j ACCEPT -m comment --comment "GitForge API" 2>/dev/null || true
iptables -D INPUT -p tcp --dport 8081 -j ACCEPT -m comment --comment "GitForge Scheduler" 2>/dev/null || true
iptables -D INPUT -p tcp --dport 8082 -j ACCEPT -m comment --comment "GitForge Git HTTP" 2>/dev/null || true
iptables -D INPUT -p tcp --dport 2222 -j ACCEPT -m comment --comment "GitForge Git SSH" 2>/dev/null || true

# Add new GitForge rules
echo "Adding new rules..."
iptables -A INPUT -p tcp --dport $API_PORT -j ACCEPT -m comment --comment "GitForge API"
iptables -A INPUT -p tcp --dport $SCHEDULER_PORT -j ACCEPT -m comment --comment "GitForge Scheduler"
iptables -A INPUT -p tcp --dport $GIT_HTTP_PORT -j ACCEPT -m comment --comment "GitForge Git HTTP"
iptables -A INPUT -p tcp --dport $GIT_SSH_PORT -j ACCEPT -m comment --comment "GitForge Git SSH"

echo ""
echo "✅ GitForge firewall rules configured!"
echo ""
echo "Ports now open:"
echo "  - $API_PORT/tcp (GitForge API)"
echo "  - $SCHEDULER_PORT/tcp (GitForge Scheduler)"
echo "  - $GIT_HTTP_PORT/tcp (GitForge Git HTTP)"
echo "  - $GIT_SSH_PORT/tcp (GitForge Git SSH)"
echo ""
echo "To persist these rules, run:"
echo "  sudo iptables-save > /etc/iptables/rules.v4  # Debian/Ubuntu"
echo "  sudo iptables-save > /etc/sysconfig/iptables  # RHEL/CentOS"
