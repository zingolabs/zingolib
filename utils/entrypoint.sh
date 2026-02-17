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

# Use setpriv to drop privileges and execute the given command as the specified UID:GID
exec_as_user() {
  user=$(id -u)
  if [[ ${user} == '0' ]]; then
    setpriv -d
    setpriv --reuid "${UID}" --regid "${GID}" --init-groups "$@"
  else
    exec "$@"
  fi
}

# Helper function
exit_error() {
  echo "$1" >&2
  exit 1
}

# Creates a default wallet directory if it doesn't exist (If the directory
# already exists, it does nothing) and sets ownership to specified UID:GID.
#
# ## Parameters
#
# - $1: Directory path to create and own
create_owned_directory() {
  local dir="$1"
  # Skip if directory is empty
  [[ -z ${dir} ]] && return

  # Create directory with parents
  mkdir -p "${dir}" || exit_error "Failed to create directory: ${dir}"

  # Set ownership for the created directory
  chown -R "${UID}:${GID}" "${dir}" || exit_error "Failed to secure directory: ${dir}"

  ls -la /usr/local/bin
  ls -la /usr/local/bin/wallets

  # Set ownership for parent directory (but not if it's root or home)
  local parent_dir
  parent_dir="$(dirname "${dir}")"
  if [[ "${parent_dir}" != "/" && "${parent_dir}" != "${HOME}" ]]; then
    chown "${UID}:${GID}" "${parent_dir}"
  fi

}

whoami
# Create and own wallet directory
[[ -n /usr/local/bin/wallets ]] && create_owned_directory "/usr/local/bin/wallets"

# Main Script Logic
#
# 1. Print environment variables and config for debugging
# 2. Process command-line arguments and execute appropriate action

echo "INFO: Using the following environment variables:"
printenv

# - If "$1" is "zingo-cli", run `zingo-cli` with the remaining provided params.
# - If "$1" is not "zingo-cli" run "$@" directly.
if [[ "$1" == "zingo-cli" ]]; then
  shift
  exec_as_user zingo-cli "$@"
else
  exec_as_user "$@"
fi

# Currently there is no support for running tests in-container, due to
# requiring additional binaries.
