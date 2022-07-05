## Regtest Mode
WARNING Experimental!
The CLI can work in regtest mode, by locally running a `zcashd` and `lightwalletd`.
This is now working with a simple zingo-cli invocation flag, with a little prior setup.

There are pre-made directories in this repo to support ready use of regtest mode. These are found in the `/regtest/` subdirectory.

Usage example:
Copy your compiled `zcashd` `zcash-cli` and `lightwalletd` binaries to `zingolib/regtest/bin/` or set up symlinks, etc.

There are default config files for these binaries already in place in `/zingolib/regtest/conf/` - which can be edited also.

From your `zingolib/` directory, with, for example, a release build (`cargo build --release`), you can run:
`./target/release/zingo-cli --regtest --data-dir regtest/datadir/zingo --server=127.0.0.1:9067`
This will start `zcashd` and `lightwalletd` and then connect to these tools with an interactive `zingo-cli`.
It currently takes about 15 seconds to do so, to give the daemons time to boot, but this will be shortened soon.
Also, please note that right now these daemons will still be running when `zingo-cli` shuts down!

You should see several diagnostic messsages, and then:
`regtest detected and network set correctly!
Lightclient connecting to http://127.0.0.1:9067/`
at which point the interactive cli application should work with your regtest network.

lwd's log file will show something like:
`{"app":"lightwalletd","level":"info","msg":"Got sapling height 1 block height 0 chain regtest ..."}`
...which you can view with `tail -f` or your favorite tool.

You may need to add blocks to your regtest chain if you have not done so previously.
In still another terminal instance in the `zingolib/regitest/bin/` directory, you can run `.zcash-cli -regtest generate 11` to generate 11 blocks.
Please note that by adding more than 100 blocks it is difficult or impossible to rewind the chain. The config means that after the first block all network upgrades should be in place.
Other `zcash-cli` commands should work similarly.

Invocation currently only works when being launched within a `zingolib` repo's worktree.
(The paths have to know where to look for the subdirectories, they start with the top level of a `zingolib` repo, or fail immediately)

Have fun!

# Details:
We have recently added support for `Network::Regtest` enum: https://github.com/zcash/librustzcash/blob/main/zcash_primitives/src/constants/regtest.rs
This has not been sufficiently tested, but seems to work well.

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
Tested with `zcash` commit `2e6a25`, `lightwalletd` commit `db2795`, and `zingolib` commit `f03d57` or better.

