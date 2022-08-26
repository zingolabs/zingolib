## Regtest Mode
WARNING Experimental!
The CLI can work in regtest mode, by locally running a `zcashd` and `lightwalletd`.
This is now working with a simple `zingo-cli` invocation flag, with a little prior setup.

There are pre-made directories in this repo to support ready use of regtest mode. These are found in the `/regtest/` subdirectory.

Usage example:
Copy your compiled `zcashd` `zcash-cli` and `lightwalletd` binaries to `zingolib/regtest/bin/` or set up symlinks, etc.

There are default config files for these binaries already in place in `/zingolib/regtest/conf/` - which can be edited also.

For example, from your `zingolib/` directory, with a binary produced from `cargo build`, you can run:
`./target/debug/zingo-cli --regtest --data-dir regtest/data/zingo --server=127.0.0.1:9067`
This will start `zcashd` and `lightwalletd` and then connect to these tools with an interactive `zingo-cli`.
It currently takes a few seconds to do so, even on a fast machine, to give the daemons time to boot.

These daemons will be killed when the user exits `zingo-cli` using the `quit` command.
If there is an issue starting or shutting down regtest mode, it's possible you will have to shut down the daemons manually.

You should see several diagnostic messsages, and then:
`regtest detected and network set correctly!
Lightclient connecting to http://127.0.0.1:9067/`
at which point the interactive cli application should work with your regtest network.

`zcashd`'s stdout logfile should quickly have an output of several dozen lines, and show network upgrade activation parameters at `height=1`.
`lightwalletd`'s log file will show something like:
`{"app":"lightwalletd","level":"info","msg":"Got sapling height 1 block height 0 chain regtest ..."}`
...which you can view with `tail -f` or your favorite tool.

Because regtest mode has an inability to cope with an initial state without any blocks we have included an initial state of one block made from `zcashd` in the `zingolib` git repo.
Additionally, any blocks added while zcashd is running are not recorded / retained upon subsequent runs.
Network parameters are all set to activate at block 1, and so all network upgrades should neccessarily be enacted when using Regtest Mode when using this branch.

Once regtest mode is running, you can manipulate the simulated chain with `zcash-cli`.

For example, in still another terminal instance in the `zingolib/regtest/bin/` directory, you can run 
`./zcash-cli -regtest -rpcuser=xxxxxx -rpcpassword=xxxxxx generate 11` to generate 11 blocks.
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
├── data
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
Tested with `zcash` commit `d6d209`, `lightwalletd` commit `f53511c`, and `zingolib` commit `90a74dd` or better.

