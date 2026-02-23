# syntax=docker/dockerfile:1
# check=skip=UndefinedVar,UserExist # We use `runuser` in the entrypoint instead of USER directive


############################
# Global build args
############################
ARG RUST_VERSION=1.91.1
ARG UID=10901
ARG GID=${UID}
ARG USER=user
ARG HOME=/home/user

############################
# Dependencies
############################
# Build Deps
FROM stagex/pallet-rust@sha256:4062550919db682ebaeea07661551b5b89b3921e3f3a2b0bc665ddea7f6af1ca AS pallet-rust
FROM stagex/user-protobuf@sha256:b399bb058216a55130d83abcba4e5271d8630fff55abbb02ed40818b0d96ced1 AS protobuf
FROM stagex/user-abseil-cpp@sha256:926f69e9cd112dfe3450a0af56d1560dc0a62589e61047e8c92c3b7edf8dd71e AS abseil-cpp
FROM stagex/core-sqlite3@sha256:44807b914585c81dda2bb0a5617cab53395255fe6685ce9599628060229c8929 AS sqlite3
# Runtime Deps
FROM stagex/core-busybox@sha256:d608daa946e4799cf28b105aba461db00187657bd55ea7c2935ff11dac237e27 AS busybox
# FROM stagex/core-llvm-runtime@sha256:11323894375bc44bc7da121345eb88a26c32edc63d0b07fdf8c08906c283751c AS llvm-runtime
# FROM stagex/core-filesystem@sha256:cd3a66471ce1f630fa77d5c9bd9829f9f9fab6302a1aaa64d67b74f1f069b750 AS filesystem
# FROM stagex/core-bash@sha256:5b598c14eef61148baf3f5a2830a214a5985b5d3544b019e3d0ed53c6b66989a AS bashy
# FROM stagex/user-util-linux@sha256:dbe8025801b4aa2ce8b7077a594ec6c5516a3f9d075283d56e9cd119631fa2c3 AS util-linux


############################
# Builder
############################
FROM pallet-rust AS builder
COPY --from=protobuf . /
COPY --from=abseil-cpp . /
COPY --from=sqlite3 . /

SHELL ["/bin/sh", "-euo", "pipefail", "-c"]
WORKDIR /usr/src/app

# Set environment variables
ENV SOURCE_DATE_EPOCH=1
ENV CARGO_HOME=/usr/local/cargo

ENV RUST_BACKTRACE=1
ENV TARGET_ARCH="x86_64-unknown-linux-musl"
ENV RUSTFLAGS="-C codegen-units=1"
ENV RUSTFLAGS="${RUSTFLAGS} -C target-feature=+crt-static"
ENV RUSTFLAGS="${RUSTFLAGS} -C link-arg=-Wl,--build-id=none"

# Copy entire workspace
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo fetch --locked --target $TARGET_ARCH

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo metadata --locked --format-version=1 > /dev/null 2>&1

# TODO : --network=none was removed due to network requests in build script
# this needs to be re-added to ensure hermeticity
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/usr/src/app/target \
    cargo build --release --frozen --target $TARGET_ARCH --bin zingo-cli && install -D -m 0755 /usr/src/app/target/${TARGET_ARCH}/release/zingo-cli /usr/local/bin/zingo-cli

############################
# Export stage
############################
FROM scratch AS export
COPY --from=builder /usr/local/bin/zingo-cli /zingo-cli

############################
# Runtime stage
############################
FROM busybox AS runtime

# Create a non-privileged user for running `zingo-cli`.
#
# We use a high UID/GID (10901) to avoid overlap with host system users.
# This reduces the risk of container user namespace conflicts with host accounts,
# which could potentially lead to privilege escalation if a container escape occurs.
#
# We do not use the `--system` flag for user creation since:
# 1. System user ranges (100-999) can collide with host system users
#   (see: https://github.com/nginxinc/docker-nginx/issues/490)
# 2. There's no value added and warning messages can be raised at build time
#   (see: https://github.com/dotnet/dotnet-docker/issues/4624)
#
# The high UID/GID values provide an additional security boundary in containers
# where user namespaces are shared with the host.
ARG UID
ENV UID=${UID}
ARG GID
ENV GID=${GID}
ARG USER
ENV USER=${USER}
ARG HOME
ENV HOME=${HOME}

COPY --chmod=644 <<-EOF /etc/passwd
	root:x:0:0:root:/root:/bin/sh
	user:x:${UID}:${GID}::${HOME}:/bin/sh
EOF

COPY --chmod=644 <<-EOF /etc/group
	root:x:0:
	user:x:${GID}:
EOF

WORKDIR /usr/local/bin
USER root
RUN mkdir -p /usr/local/bin/wallets && chown -R ${UID}:${GID} /usr/local/bin/ && chmod -R 770 /usr/local/bin/
COPY --chown=${UID}:${GID} --from=export /zingo-cli /usr/local/bin/zingo-cli
RUN chmod 550 /usr/local/bin/zingo-cli
COPY --chown=${UID}:${GID} ./utils/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod 550 /usr/local/bin/entrypoint.sh
USER $USER
ENTRYPOINT [ "./entrypoint.sh" ]
# selected server = zebra 4.1.0 and lwd v0.4.18-9-gb932e8e at time of commit
# ENTRYPOINT [ "./zingo-cli", "--server", "https://zzz.stripest.online:443", "--waitsync", "version" ]
