#!/bin/sh

# Entrypoint for running zingo-cli in Docker.
#
# The main script logic is at the bottom.

set -eo pipefail

# Currently there is no support for running tests in-container, due to
# requiring additional binaries.
#
# Main Script Logic
#
# 1. Print environment variables and config for debugging.
# 2. Tests if zingo-cli runs.
# 3. Creates a wallet, if the container has not been initialized before.
# 4. Process command-line arguments and execute appropriate action.

echo "INFO: Using the following environment variables:"
printenv

echo "Testing zingo-cli to print version string:"
./zingo-cli --version

if [ ! -f ./initialized ]; then
  # A wallet will be created in this container if there is none. The address of the new wallet will be printed after sync."
  echo "Container not initialized, creating wallet, syncing, and printing address..."
  # selected server = zebra 4.1.0 and lwd v0.4.18-9-gb932e8e at time of commit
  ./zingo-cli --server https://zzz.stripest.online:443 --waitsync addresses
  touch ./initialized
fi

echo "Lightwalletd server's info info:"
./zingo-cli --server https://zzz.stripest.online:443 --nosync info

echo "now exec'ing $@ "
exec "$@"
