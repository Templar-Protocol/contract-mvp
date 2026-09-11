# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.92.0
ARG WORKSPACE_RUST_VERSION=1.86.0
ARG SOROBAN_RUST_VERSION=1.89.0
ARG STELLAR_CLI_VERSION=26.0.0

FROM rust:${RUST_VERSION}-bookworm AS stellar-cli-builder
ARG STELLAR_CLI_VERSION

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        libdbus-1-dev \
        libudev-dev \
        pkg-config && \
    rm -rf /var/lib/apt/lists/*

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git/db,sharing=locked \
    cargo install --locked "stellar-cli@${STELLAR_CLI_VERSION}"

FROM rust:${RUST_VERSION}-bookworm AS cli-builder
ARG WORKSPACE_RUST_VERSION

WORKDIR /usr/src/templar

RUN rustup toolchain install "${WORKSPACE_RUST_VERSION}" --profile minimal && \
    rustup target add wasm32-unknown-unknown --toolchain "${WORKSPACE_RUST_VERSION}"

COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/usr/local/cargo/git/db,sharing=locked \
    cargo build --locked --release -p templar-soroban-vault-cli --bin tmplr-soroban-vault && \
    strip target/release/tmplr-soroban-vault

FROM rust:${RUST_VERSION}-bookworm AS runtime
ARG WORKSPACE_RUST_VERSION
ARG SOROBAN_RUST_VERSION
ARG STELLAR_CLI_VERSION

LABEL org.opencontainers.image.title="Templar Soroban Vault CLI"
LABEL org.opencontainers.image.description="Operator image for deploying and managing Templar Soroban vault stacks"
LABEL org.opencontainers.image.vendor="Templar Protocol"
LABEL org.opencontainers.image.licenses="GPL-3.0-only"
LABEL org.opencontainers.image.source="https://github.com/Templar-Protocol/contracts"
LABEL org.opencontainers.image.documentation="https://github.com/Templar-Protocol/contracts/tree/dev/tools/soroban-vault-cli"
LABEL org.opencontainers.image.stellar-cli.version="${STELLAR_CLI_VERSION}"

RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        libdbus-1-3 \
        libdbus-1-dev \
        libssl3 \
        libudev-dev \
        libudev1 \
        pkg-config \
        python3 && \
    rm -rf /var/lib/apt/lists/*

RUN rustup toolchain install "${WORKSPACE_RUST_VERSION}" "${SOROBAN_RUST_VERSION}" --profile minimal && \
    rustup target add wasm32-unknown-unknown --toolchain "${WORKSPACE_RUST_VERSION}" && \
    rustup target add wasm32-unknown-unknown --toolchain "${SOROBAN_RUST_VERSION}" && \
    rustup target add wasm32-unknown-unknown

RUN groupadd --system templar && \
    useradd --system --gid templar --create-home --home-dir /home/templar templar

COPY --from=stellar-cli-builder /usr/local/cargo/bin/stellar /usr/local/bin/stellar
COPY --from=cli-builder /usr/src/templar/target/release/tmplr-soroban-vault /usr/local/bin/tmplr-soroban-vault

WORKDIR /workspace
# The source tree and toolchains remain available for explicit `--build`;
# normal deployment resolves a verified pinned release artifact. .dockerignore
# keeps VCS metadata, local build outputs, and secrets out of the context.
COPY --chown=templar:templar . /workspace

RUN mkdir -p \
        /home/templar/.cache/templar/soroban-vault-cli/artifacts \
        /home/templar/.cargo \
        /home/templar/.config/stellar \
        /workspace/contract/vault/soroban/.deploy-state \
        /workspace/target && \
    chown -R templar:templar /home/templar /workspace

ENV CARGO_HOME=/home/templar/.cargo
ENV RUSTUP_HOME=/usr/local/rustup
ENV RUSTUP_TOOLCHAIN=${SOROBAN_RUST_VERSION}
ENV SOROBAN_NETWORK=testnet
ENV STELLAR_CONFIG_DIR=/home/templar/.config/stellar
ENV TEMPLAR_SOROBAN_VAULT_STATE=/workspace/contract/vault/soroban/.deploy-state/manifest.json
ENV WORKSPACE_PATH=/workspace
ENV TEMPLAR_SOROBAN_VAULT_ARTIFACT_CACHE=/home/templar/.cache/templar/soroban-vault-cli/artifacts

# These volumes persist operator state, verified release artifacts, and build
# caches. Mounts for /workspace/target or .deploy-state shadow copied image paths.
VOLUME ["/home/templar/.cache/templar/soroban-vault-cli/artifacts", "/home/templar/.cargo", "/home/templar/.config/stellar", "/workspace/contract/vault/soroban/.deploy-state", "/workspace/target"]

USER templar

RUN stellar version >/dev/null && tmplr-soroban-vault --version >/dev/null

ENTRYPOINT ["tmplr-soroban-vault"]
CMD ["--help"]
