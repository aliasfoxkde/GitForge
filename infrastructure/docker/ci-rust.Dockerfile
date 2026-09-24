# Pre-warmed Rust CI image for the local GitForge pipeline (gitforge-ci).
#
# The pipeline's jobs run offline behind CARGO_NET_OFFLINE=true (crates.io
# transfers from this network stall mid-run), so the crate registry for the
# committed Cargo.lock is baked in and jobs start compiling immediately.
# When the lockfile changes (e.g. an audited-dependency bump) the cache no
# longer matches and every cargo job fails in seconds with "candidate
# versions found which didn't match" — that is the signal to rebuild and
# bump the tag.
#
# Rebuild (from a clean checkout of the commit whose Cargo.lock you want
# baked; the manifest-and-source-only context keeps the 30GB+ target/
# directory out of the build):
#
#   tmp=$(mktemp -d)
#   git archive HEAD Cargo.toml Cargo.lock crates services \
#       | tar -x -C "$tmp"
#   cp infrastructure/docker/ci-rust.Dockerfile "$tmp"/
#   docker build -f ci-rust.Dockerfile -t dsc-ci-rust:N "$tmp"
#
# Then update the `image:` fields in .gitforge.yml to the new tag. The image
# is shared with the dsc pipeline's toolchain naming, hence the dsc-ci-rust
# prefix; the cache content is GitForge's workspace.
#
# Size guard: the NAS daemon uses the vfs storage driver, where every
# `docker create` copies the image's full layer stack, and the runner has a
# hard 60-second sandbox-acquisition cap. Keep the image under ~2.5GB.
FROM rust:1-slim-bookworm

# openssl-sys needs pkg-config + libssl headers; git is a test dependency
# (ssh/https protocol suites drive a real git client) and openssh-client
# supplies ssh-keygen, which the hermetic git_ssh_protocol suite spawns.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev curl ca-certificates git openssh-client \
    && rm -rf /var/lib/apt/lists/*

# Slim images ship the minimal rustup profile; the pipeline needs both linters.
RUN rustup component add rustfmt clippy

# Toolchain contract (F25): the toolchain is frozen at image build time.
# Jobs run with no egress — `rustup toolchain install` inside a job stalls
# on the download and dies mid-run, which is exactly how run 6d5bac16
# failed. The runner now refuses such steps at preflight, so the only way
# to change the toolchain is to change it HERE, rebuild, and bump the tag:
#
#   RUN rustup toolchain install 1.95 --profile minimal
#   ENV RUSTUP_TOOLCHAIN=1.95
#
# Remember the rust-toolchain.toml files in job workspaces override
# RUSTUP_TOOLCHAIN and will trigger a silent download; pin them to the
# baked version.

# Warm the crate registry from the committed lockfile so runtime jobs never
# hit crates.io. Must come before the offline flag: this is the one step
# that is allowed to use the network.
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY services/ services/
RUN cargo fetch --locked

# Once the cache is warm, any crate that escapes it means the image is stale
# relative to Cargo.lock — fail the job in seconds with a clear message
# instead of hanging on a stalled download for minutes.
ENV CARGO_NET_OFFLINE=true \
    CARGO_TERM_COLOR=always
