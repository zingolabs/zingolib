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
# 1. Print environment variables and config for debugging
# 2. Process command-line arguments and execute appropriate action

echo "INFO: Using the following environment variables:"
printenv

echo "starting zingo-cli. A wallet will be created in this container if there is none. A version will be printed after sync."
./zingo-cli --server https://zzz.stripest.online:443 --waitsync version

# Keep container running for re-attach
echo "--  This container has succeeded in making a wallet!"
echo "Use 'docker exec -it <container> /bin/sh' in another active terminal for manual use."
# TODO add handle for manual commands
echo "'tail' executing which will keep this container running until stopped."
echo "3 SIGTERM/SIGINTs will forcefully exit."
echo "Restarting the same container will re-sync, but retain the existing wallet."
exec tail -f /dev/null
