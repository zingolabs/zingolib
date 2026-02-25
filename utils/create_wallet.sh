#!/bin/sh

set -e

echo "Checking local docker image store to see if a zingo-cli:latest image is present."
# Checks for empty string, discarding error messages.
if [ -z "$(docker images -q zingo-cli:latest 2>/dev/null)" ]; then
  echo "There is no zingo-cli:latest image listed by docker."
else
  echo "Creating wallet if there is none, then printing wallet's orchard u address."
  docker run zingo-cli:latest
  # ./zingo-cli --server https://zzz.stripest.online:443 --waitsync addresses
fi
