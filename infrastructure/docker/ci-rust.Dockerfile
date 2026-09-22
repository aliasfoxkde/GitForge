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
# (ssh/https protocol suites drive a real git client).
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev curl ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

# Slim images ship the minimal rustup profile; the pipeline needs both linters.
RUN rustup component add rustfmt clippy

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
