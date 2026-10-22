# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.95
# Build on each target platform, using native workers or BuildKit's QEMU support.
FROM rust:${RUST_VERSION}-bookworm AS build
ARG TARGETARCH
ARG RUST_VERSION
ARG PROTOC_VERSION=35.1

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates clang cmake curl libclang-dev libssl-dev mold \
        pkg-config unzip \
    && rm -rf /var/lib/apt/lists/*

# Match the protoc version used by .github/actions/prepare-build/action.yml.
RUN set -eu; \
    case "$TARGETARCH" in \
        amd64) protoc_arch=x86_64 ;; \
        arm64) protoc_arch=aarch_64 ;; \
        *) echo "Unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL --retry 3 \
        "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${protoc_arch}.zip" \
        -o /tmp/protoc.zip; \
    unzip -q /tmp/protoc.zip -d /usr/local; \
    rm /tmp/protoc.zip; \
    test "$(protoc --version)" = "libprotoc ${PROTOC_VERSION}"

WORKDIR /src
COPY . .
ENV CARGO_TARGET_DIR=/build/target
# Keep the output outside the cache mount so later stages can copy it.
RUN --mount=type=cache,id=easytier-registry-${TARGETARCH},target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=easytier-git-${TARGETARCH},target=/usr/local/cargo/git,sharing=locked \
    --mount=type=cache,id=easytier-target-${RUST_VERSION}-${TARGETARCH},target=/build/target,sharing=locked \
    cargo build --locked --release --package easytier \
        --bin easytier-core \
    && mkdir -p /out \
    && cp /build/target/release/easytier-core /out/

# Export only executables, without the builder or runtime filesystem.
FROM scratch AS binaries
COPY --from=build /out/ /

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates iproute2 libgcc-s1 libssl3 libstdc++6 tini tzdata \
    && rm -rf /var/lib/apt/lists/*
ENV TZ=Asia/Shanghai
WORKDIR /app
COPY --from=build /out/ /usr/local/bin/
EXPOSE 11010/tcp 11010/udp 11011/tcp 11011/udp 11012/tcp
ENTRYPOINT ["/usr/bin/tini", "--", "easytier-core"]
CMD ["-c", "/etc/easytier/config.toml"]
