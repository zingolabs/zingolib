#!/bin/sh

set -e

REPO_ROOT="$(git rev-parse --show-toplevel)"
OCI_OUTPUT="$REPO_ROOT/build/oci"
TARBALL="${OCI_OUTPUT}/zingo-cli.tar"

# Build runtime image for docker run
echo "Checking if the OCI output from build is present."
if [ -f "$TARBALL" ];
then
  echo "OCI output file present, loading tar file into local docker image store."
  docker load < $TARBALL
  echo "...Done!"
else
  echo "OCI output file not present."
fi
