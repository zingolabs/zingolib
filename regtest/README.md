## Regtest
WARNING Experimental!
The CLI can work in regtest mode, by locally running a `zcashd` and `lightwalletd`.

We have recently added support for `Network::Regtest` enum: https://github.com/zcash/librustzcash/blob/main/zcash_primitives/src/constants/regtest.rs
This has not been sufficiently tested, but now compiles.

There are pre-made directories in this repo to support ready use of regtest mode. These are found in the `/regtest/` subdirectory.

This documentation to run regtest "manually" is a placeholder until flags are more functional. (https://github.com/zingolabs/zingolib/issues/17)

For example:
Copy your compiled `zcashd` `zcash-cli` and `lightwalletd` binaries to `zingolib/regtest/bin/` or set up symlinks, etc.

There are default config files for these binaries already in place in `/zingolib/regtest/conf/`

From `/zingolib/regtest/bin/`, `zcashd` works in regtest with the following invocation (with absolute pathes - you will have to adjust to your own path):
`./zcashd --printtoconsole -conf=/zingolib/regtest/conf/zcash.conf --datadir=/zingolib/regtest/datadir/zcash/ -debuglogfile=/zingolib/regtest/logs/debug.log -debug=1`

From `/zingolib/regtest/bin/`, in another terminal instance, run:
`./lightwalletd --no-tls-very-insecure --zcash-conf-path ../conf/zcash.conf --config ../conf/lightwalletdconf.yml --data-dir ../datadir/lightwalletd/--log-file ../logs/lwd.log`
`lightwalletd`'s terminal output will not have clear success, but the log file will show something like:
`{"app":"lightwalletd","level":"info","msg":"Got sapling height 1 block height 0 chain regtest ..."}`
...which you can view with `tail -f` or your favorite tool.

You will need to add blocks to your regtest chain if you have not done so previously.
In still another terminal instance in the `zingolib/regitest/bin/` directory, you can run `.zcash-cli -regtest generate 11` to generate 11 blocks.
Please note that by adding more than 100 blocks it is difficult or impossible to rewind the chain. The config means that after the first block all network upgrades should be in place.

Finally, from your `zingolib/` directory, with, for example, a release build (`cargo build --release`), you can run:
`./target/release/zingo-cli --regtest --data-dir regtest/datadir/zingo --server=127.0.0.1:9067`

You should see a single line printed out saying `regtest detected and network set correctly!` and the interactive cli application should work with your regtest network!

# Tree Diagrams

In `/zingolib/`, running `tree ./regtest`
Before moving binaries or running:
regtest/
├── bin
├── conf
│   ├── lightwalletdconf.yml
│   └── zcash.conf
├── datadir
│   ├── lightwalletd
│   ├── zcash
│   └── zingo
├── logs
└── README.md

after moving binaries and running:
regtest/
├── bin
│   ├── lightwalletd
│   ├── zcash-cli
│   └── zcashd
├── conf
│   ├── lightwalletdconf.yml
│   └── zcash.conf
├── datadir
│   ├── lightwalletd
│   │   └── db
│   │       └── regtest
│   │           ├── blocks
│   │           └── lengths
│   ├── zcash
│   │   └── regtest
│   │       ├── banlist.dat
│   │       ├── blocks
│   │       │   ├── blk00000.dat
│   │       │   ├── index
│   │       │   │   ├── 000003.log
│   │       │   │   ├── CURRENT
│   │       │   │   ├── LOCK
│   │       │   │   ├── LOG
│   │       │   │   └── MANIFEST-000002
│   │       │   └── rev00000.dat
│   │       ├── chainstate
│   │       │   ├── 000003.log
│   │       │   ├── CURRENT
│   │       │   ├── LOCK
│   │       │   ├── LOG
│   │       │   └── MANIFEST-000002
│   │       ├── db.log
│   │       ├── fee_estimates.dat
│   │       ├── peers.dat
│   │       └── wallet.dat
│   └── zingo
│       ├── zingo-wallet.dat
│       └── zingo-wallet.debug.log
├── logs
│   └── lwd.log
└── README.md

# Working Commits
Tested with `zcash` commit `2e6a25`, `lightwalletd` commit `db2795`, and `zingolib` commit `979f82` or better.

