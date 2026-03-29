# syntax=docker/dockerfile:1
# check=skip=UndefinedVar,UserExist

# stages:
# - release: setup and builds release binaries
# - export: discrete stage for writing binaries into host build directory
# - runtime: prepares the release image
#
# We first set default values for build arguments used across the stages.
# Each stage must define the build arguments (ARGs) it uses.

ARG FEATURES=""

############################
# Global build args
############################
ARG UID=10901
ARG GID=${UID}
ARG USER="user"
ARG HOME="/home/${USER}"
ARG CARGO_HOME="/usr/local/.cargo"
ARG CARGO_TARGET_DIR="${HOME}/target"
ARG TARGET_ARCH="x86_64-unknown-linux-musl"

############################
# Dependencies
############################
# Build Deps
FROM stagex/pallet-rust:1.94.0@sha256:2fbe7b164dd92edb9c1096152f6d27592d8a69b1b8eb2fc907b5fadea7d11668 AS pallet-rust
FROM stagex/user-protobuf:26.1@sha256:a135aaf060990b6ef8a7c715c16f175811d3a1f5383970f5771adef05a0bc56a AS protobuf
FROM stagex/user-abseil-cpp:20240116.2@sha256:20a241145158a0aa7cb83ed5dc4f9ad6360dc975352787f4e6b00e8a39943f62 AS abseil-cpp
FROM stagex/core-sqlite3:3.50.1@sha256:8d2959fcde94119a724315d9c9f58acf59c5ae83cf4ad22a36ac1ed971327237 AS sqlite3
# Runtime Deps
FROM stagex/core-busybox:1.37.0@sha256:d608daa946e4799cf28b105aba461db00187657bd55ea7c2935ff11dac237e27 AS busybox


############################
# Release
############################
FROM pallet-rust AS release
COPY --from=protobuf . /
COPY --from=abseil-cpp . /
COPY --from=sqlite3 . /

SHELL ["/bin/sh", "-euo", "pipefail", "-c"]

ARG HOME
WORKDIR ${HOME}

ARG CARGO_INCREMENTAL
# default to 0, disables incremental compilation.
ENV CARGO_INCREMENTAL=${CARGO_INCREMENTAL:-0}

ARG CARGO_HOME
ENV CARGO_HOME=${CARGO_HOME}

ARG CARGO_TARGET_DIR
ARG TARGET_ARCH

ARG FEATURES
ENV FEATURES=${FEATURES}

ENV RUST_BACKTRACE=1
ENV RUSTFLAGS="-C codegen-units=1"
ENV RUSTFLAGS="${RUSTFLAGS} -C target-feature=+crt-static"
ENV RUSTFLAGS="${RUSTFLAGS} -C link-arg=-Wl,--build-id=none"

ENV SOURCE_DATE_EPOCH=1

# Copy entire workspace
COPY . .

RUN --mount=type=cache,target=${CARGO_HOME}/registry \
    --mount=type=cache,target=${CARGO_HOME}/git \
    cargo fetch --locked --target $TARGET_ARCH

RUN --mount=type=cache,target=${CARGO_HOME}/registry \
    --mount=type=cache,target=${CARGO_HOME}/git \
    cargo metadata --locked --format-version=1 > /dev/null 2>&1

# TODO : --network=none was removed due to network requests in build script
# this needs to be re-added to ensure hermeticity
RUN --mount=type=cache,target=/${CARGO_HOME}registry \
    --mount=type=cache,target=${CARGO_HOME}/git \
    --mount=type=cache,target=${HOME}/target \
    cargo build --release --frozen --target $TARGET_ARCH --bin zingo-cli && install -D -m 0755 /usr/src/app/target/${TARGET_ARCH}/release/zingo-cli /usr/local/bin/zingo-cli

############################
# Export stage
############################
FROM scratch AS export
COPY --from=release /usr/local/bin/zingo-cli /zingo-cli

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

COPY --chmod=550 <<-EOF /etc/passwd
	root:x:0:0:root:/root:/bin/sh
	user:x:${UID}:${GID}::${HOME}:/bin/sh
EOF

COPY --chmod=550 <<-EOF /etc/group
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
# ./entrypoint.sh runs, then executes CMD (or custom command if provided).
# Prints zingo-cli version, address if a new wallet is created, and info on success.
ENTRYPOINT [ "./entrypoint.sh" ]
CMD [ "./zingo-cli", "--help"]
