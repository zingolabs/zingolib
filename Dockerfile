# syntax=docker/dockerfile:1

############################
# Global build args
############################
ARG RUST_VERSION=1.91.1
ARG UID=1000
ARG GID=1000
ARG USER=container_user
ARG HOME=/home/container_user

############################
# Dependencies
############################
FROM stagex/pallet-rust@sha256:4062550919db682ebaeea07661551b5b89b3921e3f3a2b0bc665ddea7f6af1ca AS pallet-rust
FROM stagex/user-protobuf@sha256:b399bb058216a55130d83abcba4e5271d8630fff55abbb02ed40818b0d96ced1 AS protobuf
FROM stagex/user-abseil-cpp@sha256:926f69e9cd112dfe3450a0af56d1560dc0a62589e61047e8c92c3b7edf8dd71e AS abseil-cpp

# the old way : FROM stagex/core-user-runtime
FROM stagex/core-llvm-runtime@sha256:11323894375bc44bc7da121345eb88a26c32edc63d0b07fdf8c08906c283751c AS llvm-runtime

############################
# Builder
############################
FROM pallet-rust AS builder
COPY --from=protobuf . /
COPY --from=abseil-cpp . /

SHELL ["/bin/sh", "-euo", "pipefail", "-c"]
WORKDIR /usr/src/app

# Set environment variables
ENV SOURCE_DATE_EPOCH=1
# ENV CXXFLAGS="-include cstdint"
# ENV ROCKSDB_USE_PKG_CONFIG=0
ENV CARGO_HOME=/usr/local/cargo

ENV RUST_BACKTRACE=1
# this target should be linked staticly by default
ENV TARGET_ARCH="x86_64-unknown-linux-musl"
ENV RUSTFLAGS="-C codegen-units=1"
# ENV RUSTFLAGS="${RUSTFLAGS} -C target-feature=+crt-static"
ENV RUSTFLAGS="${RUSTFLAGS} -C link-arg=-Wl,--build-id=none"

# Copy entire workspace
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch --locked --target $TARGET_ARCH

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo metadata --locked --format-version=1 > /dev/null 2>&1

RUN --network=none \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/app/target \
    # Q: just `install` here
    cargo build --release --frozen --target $TARGET_ARCH --bin zingo-cli && install -D -m 0755 /usr/src/app/target/${TARGET_ARCH}/release/zingo-cli /usr/local/bin/zaino-cli

############################
# Export stage
############################
FROM scratch AS export
COPY --from=builder /usr/local/bin/zingo-cli /zingo-cli

############################
# Runtime stage
# (slim, non-root)?
############################
FROM llvm-runtime AS runtime

ARG HOME

WORKDIR ${HOME}

# Copy the installed binary from builder
COPY --from=export /zingo-cli /

RUN /zingo-cli --version
RUN /usr/local/bin/zingo-cli --version

#HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
#  CMD /usr/local/bin/zingo-cli --version >/dev/null 2>&1 || exit 1

ENTRYPOINT ["/zingo-cli"]
CMD []
