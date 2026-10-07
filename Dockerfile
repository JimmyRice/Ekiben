# syntax=docker/dockerfile:1.7

# Kippu server image.
#
# The binary is cross-compiled on the build host's own architecture into a static musl
# executable (cargo-zigbuild), so a multi-platform build needs no emulation, and the runtime
# image is distroless/static: CA certificates, /etc/passwd and tzdata, no libc, no shell.
#
#   docker build -t kippu .
#   docker build -t kippu --build-arg FEATURES=postgres,nats,s3,mimalloc .
#   docker buildx build --platform linux/amd64,linux/arm64 -t kippu .
#
# FEATURES is the `ekiben` feature list from the root Cargo.toml (sqlite, postgres, mysql,
# nats, s3, gcs, azure, base45, mimalloc). Default features are off, so only what is named is
# compiled in. Keep `mimalloc`: musl's own allocator is slow under a multi-threaded server.

ARG RUST_VERSION=1.98.1

# ── Toolchain: Rust, Zig and cargo-zigbuild, on the build host's architecture ────
FROM --platform=$BUILDPLATFORM docker.io/library/rust:${RUST_VERSION}-trixie AS toolchain

ARG RUST_VERSION
ARG ZIG_VERSION=0.15.2
ARG CARGO_ZIGBUILD_VERSION=0.23.4
ARG BUILDARCH

SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

# Use the image's toolchain: `rust-toolchain.toml` asks for `stable`, which rustup would
# otherwise download (with rustfmt and clippy) on the first cargo call.
ENV RUSTUP_TOOLCHAIN=${RUST_VERSION}

RUN case "${BUILDARCH}" in \
        amd64) arch=x86_64;  sha256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239 ;; \
        arm64) arch=aarch64; sha256=958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f ;; \
        *) echo "unsupported build architecture: ${BUILDARCH}" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/zig.tar.xz "https://ziglang.org/download/${ZIG_VERSION}/zig-${arch}-linux-${ZIG_VERSION}.tar.xz"; \
    echo "${sha256}  /tmp/zig.tar.xz" | sha256sum -c -; \
    mkdir /opt/zig; \
    tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1; \
    ln -s /opt/zig/zig /usr/local/bin/zig; \
    rm /tmp/zig.tar.xz

RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    cargo install --locked "cargo-zigbuild@${CARGO_ZIGBUILD_VERSION}"

# ── Build ───────────────────────────────────────────────────────────────────────
FROM toolchain AS build

ARG TARGETARCH
ARG FEATURES=sqlite,mimalloc
ARG PROFILE=dist

WORKDIR /src
ENV CARGO_TARGET_DIR=/build/target

# The source is bind-mounted read-only, so it never becomes a layer; the registry and the
# target directory are cache mounts that survive between builds (one target per architecture,
# so a multi-platform build does not serialize on it). The binary is copied out of the cache.
RUN --mount=type=bind,target=/src \
    --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=cargo-git,target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=kippu-target-${TARGETARCH},target=/build/target \
    case "${TARGETARCH}" in \
        amd64) target=x86_64-unknown-linux-musl ;; \
        arm64) target=aarch64-unknown-linux-musl ;; \
        *) echo "unsupported target architecture: ${TARGETARCH}" >&2; exit 1 ;; \
    esac; \
    cargo zigbuild --locked \
        --target "${target}" \
        --profile "${PROFILE}" \
        --no-default-features --features "${FEATURES}"; \
    install -D -m 0555 "${CARGO_TARGET_DIR}/${target}/${PROFILE}/kippu" /out/kippu

# The data directory for SQLite (`sqlite:///var/lib/kippu/kippu.db`), owned by distroless's
# `nonroot` user; the runtime image has no shell to create it.
RUN install -d -m 0750 -o 65532 -g 65532 /out/data

# ── Runtime ─────────────────────────────────────────────────────────────────────
FROM gcr.io/distroless/static-debian13:nonroot AS runtime

ARG VERSION=0.1.0
ARG REVISION=unknown
ARG FEATURES=sqlite,mimalloc

LABEL org.opencontainers.image.title="kippu" \
      org.opencontainers.image.description="Kippu, the stateless ticketing backend of Ekiben" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}" \
      dev.ekiben.kippu.features="${FEATURES}"

COPY --from=build /out/kippu /usr/local/bin/kippu
COPY --from=build --chown=65532:65532 /out/data /var/lib/kippu

# Configuration comes from KIPPU_* environment variables, or a mounted file named by
# KIPPU_CONFIG (see kippu.example.toml). The server listens on 0.0.0.0:8080 by default and
# shuts down gracefully on SIGTERM. There is no HEALTHCHECK: the image carries no HTTP
# client; probe GET /healthz and /readyz from the orchestrator.
USER 65532:65532
VOLUME ["/var/lib/kippu"]
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/kippu"]
CMD ["serve"]
