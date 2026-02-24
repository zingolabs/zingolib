#!/bin/sh

# Entrypoint for running zingo-cli in Docker.
#
# The main script logic is at the bottom.
#
# ## Notes
#
# zingo-cli runs with several defaults.
# Importantly, these include a data-dir with wallet file,
# which are created if they don't already exist:
#   a `wallets` dir in location where executable is run,
#   containing the wallet (`zingo-wallet.dat`) file.
# other defaults inlcude setting the chain to mainnet,
# using a default lightwallet server, using clearnet for price fetching,
# and not executing commands prior to a complete chain sync.

set -eo pipefail

# Currently there is no support for running tests in-container, due to
# requiring additional binaries.
#
# Main Script Logic
#
# 1. Print environment variables and config for debugging.
# 2. Creates a wallet, if the container has not been initialized before.
# 3. Tests zingo-cli.
# 4. Process command-line arguments and execute appropriate action.

echo "INFO: Using the following environment variables:"
printenv

if [ ! -f ./initialized ]; then
  # A wallet will be created in this container if there is none. A version will be printed after sync."
  # selected server = zebra 4.1.0 and lwd v0.4.18-9-gb932e8e at time of commit
  echo "Container not initialized, creating wallet, syncing, and printing address..."
  ./zingo-cli --server https://zzz.stripest.online:443 --waitsync addresses
  touch ./initialized
fi

echo "Testing zingo-cli to print version string:"
./zingo-cli --version

echo "now exec'ing $@ "
exec "$@"
