# GitForge user-scoped systemd deployment

These units are staged templates for the remote Fedora workbench. They are not
enabled by the repository checkout and do not contain credentials.

## Install as the `mkinney` user

```bash
install -Dm600 deploy/systemd/gitforge.env.example \
  "$HOME/.config/gitforge/gitforge.env"
install -Dm644 deploy/systemd/gitforge-scheduler.service \
  "$HOME/.config/systemd/user/gitforge-scheduler.service"
install -Dm644 deploy/systemd/gitforge-runner.service \
  "$HOME/.config/systemd/user/gitforge-runner.service"
systemctl --user daemon-reload
```

Review and edit the environment file before starting anything. The candidate
release binaries must exist at `target/release/` and the scheduler port must be
free. Validate first:

```bash
systemd-analyze --user unit-paths
systemd-analyze verify "$HOME/.config/systemd/user/gitforge-scheduler.service"
systemd-analyze verify "$HOME/.config/systemd/user/gitforge-runner.service"
```

Only after the loopback canary and Podman job qualification pass should the
units be enabled:

```bash
systemctl --user enable --now gitforge-scheduler.service
systemctl --user enable --now gitforge-runner.service
```

The runner unit requires the rootless Podman socket and points the Docker
client boundary at that socket. It must not be changed to a host-wide Docker
socket without an explicit security review.
