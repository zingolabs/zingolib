# zcash_local_net

## Overview

Utilities that launch and manage Zcash processes. This is used for integration
testing in the development of:

  - lightclients
  - indexers
  - validators


## List of Managed Processes
- Zebrad
- Zcashd
- Zainod
- Lightwalletd

## Prerequisites

An internet connection will be needed (during the fist build at least) in order to fetch the required testing binaries.
The binaries will be automagically checked and downloaded on `cargo build/check/test`. If you specify `None` in a process `launch` config, these binaries will be used.
The path to the binaries can be specified when launching a process. In that case, you are responsible for compiling the needed binaries.
Each processes `launch` fn and [`crate::LocalNet::launch`] take config structs for defining parameters such as path
locations.
See the config structs for each process in validator.rs and indexer.rs for more details.

### Launching multiple processes

See [`crate::LocalNet`].

## Testing

See [`crate::test_fixtures`] doc comments for running client rpc tests from external crates for indexer/validator development.

Test should be run with the `test_fixtures` feature enabled.


Current version: 0.1.0

License: MIT License
