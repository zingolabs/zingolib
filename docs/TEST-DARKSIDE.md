
## WARNING! ZingoProxy
* Using this software (a lightwalletd proxy client) does not offer the full privacy or security of running a full-node zcashd node locally.
* This software does not provide privacy guarantees against network monitoring of the type or pattern of traffic it generates. That is to say, in some cases, the specifics of use may be able to remain private, but the use of this tool may be apparent to network observers.

## Running Darksidewalletd tests.

Darkside mode of Lightwalletd lets developers create a synthetic compact blockchain to test different scenarios from a simple sync to complex things like re-orgs that sway away knowingly confirmed transactions.

### Requirements
Clients running darksidewalletd tests need to run a lightwalletd client locally, because darkside mode isn't meant to be deployed anywhere and it has safeguards so that nobody deploys a lightwalletd server in darkside mode by mistake.

lightwalletd supported for these tests need to have the TreeState API. This means from commit `5d174f7feb702dc19aec5b09d8be8b3d5b17ce45` onwards.

## running lightwalletd in darkside mode
````zsh
#!/bin/zsh

./lightwalletd --log-file /dev/stdout --darkside-very-insecure  --darkside-timeout 1000 --gen-cert-very-insecure --data-dir . --no-tls-very-insecure
````

## running the tests

cargo test --package zingo-cli --test integration_tests --features darkside_tests -- darkside --nocapture

# or
cargo test --package zingo-cli --test integration_tests --features darkside_tests -- TESTNAME --nocapture
