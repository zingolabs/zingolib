#!/bin/sh

set -e

# Build runtime image for docker run
echo "Checking if the OCI output from build is present."
if [ -z "$$(docker images -q zingo-cli:latest 2>/dev/null)" ]; then
  echo "There is no `zingo-cli:latest` image listed by docker."
else
  echo "Creating wallet if there is none, then opening zingo-cli interactively."
  docker run -it zingo-cli:latest ./zingo-cli --server https://zzz.stripest.online:443
fi
