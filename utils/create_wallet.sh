#!/bin/sh

set -e

DIR="$( cd "$( dirname "$0" )" && pwd )"
REPO_ROOT="$(git rev-parse --show-toplevel)"
PLATFORM="linux/amd64"
OCI_OUTPUT="$REPO_ROOT/build/oci"

export DOCKER_BUILDKIT=1
export SOURCE_DATE_EPOCH=1

# Build runtime image for docker run
echo "Creating wallet if there is none."
docker load < $OCI_OUTPUT/zingo-cli.tar
docker run zingo-cli:latest ./zingo-cli --server https://zzz.stripest.online:443 --waitsync addresses
