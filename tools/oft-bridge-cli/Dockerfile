# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.86.0

FROM rust:${RUST_VERSION}-bookworm AS builder

ARG RUST_VERSION
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

WORKDIR /usr/src/templar

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git/db,sharing=locked \
    cargo build --locked --release \
        -p templar-oft-bridge-cli \
        --bin tmplr-oft-bridge && \
    strip target/release/tmplr-oft-bridge

FROM debian:bookworm-slim AS runtime

LABEL org.opencontainers.image.title="Templar OFT Bridge CLI"
LABEL org.opencontainers.image.description="Operator image for Templar LayerZero OFT routes"
LABEL org.opencontainers.image.vendor="Templar Protocol"
LABEL org.opencontainers.image.licenses="GPL-3.0-only"
LABEL org.opencontainers.image.source="https://github.com/Templar-Protocol/contracts"
LABEL org.opencontainers.image.documentation="https://github.com/Templar-Protocol/contracts/tree/dev/tools/oft-bridge-cli"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 && \
    rm -rf /var/lib/apt/lists/*

RUN groupadd --system --gid 10001 templar && \
    useradd --system --uid 10001 --gid templar \
        --create-home --home-dir /home/templar templar && \
    mkdir -p /data/routes /data/operations /run/templar-secrets && \
    chown -R templar:templar \
        /home/templar /data/routes /data/operations /run/templar-secrets && \
    chmod 0700 /data/routes /data/operations /run/templar-secrets

COPY --from=builder \
    /usr/src/templar/target/release/tmplr-oft-bridge \
    /usr/local/bin/tmplr-oft-bridge

# Artifact verification and artifact-build planning resolve these immutable
# inputs through the compile-time CARGO_MANIFEST_DIR path.
COPY --from=builder \
    /usr/src/templar/tools/oft-bridge-cli/artifacts.lock.json \
    /usr/src/templar/tools/oft-bridge-cli/artifacts.lock.json
COPY --from=builder \
    /usr/src/templar/tools/oft-bridge-cli/evm \
    /usr/src/templar/tools/oft-bridge-cli/evm

WORKDIR /data

VOLUME ["/data/routes", "/data/operations"]

USER templar

RUN tmplr-oft-bridge --version >/dev/null

ENTRYPOINT ["tmplr-oft-bridge"]
CMD ["--help"]
