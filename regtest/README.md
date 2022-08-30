## Regtest Mode
WARNING Experimental!
The CLI can work in regtest mode, by locally running a `zcashd` and `lightwalletd`.
This is now working with a simple `zingo-cli` invocation flag, with a little user setup.

There are pre-made directories in this repo to support ready use of regtest mode. These are found in the `/regtest/` subdirectory.

There are default config files for these binaries already in place in `/zingolib/regtest/conf/` which can also be edited.

Because regtest mode has no ability to cope with an initial `zcashd` state without any blocks, 
we have included files to produce an initial block height of one, with no variation between runs.
These files are copied from a 'passive' directory (`zingolib/regtest/data/regtestvectors/`)
into a newly cleared 'active' data directory at the beginning of each time regtest mode is run.
This means, by default, any blocks added while zcashd is running are not retained for subsequent runs.

The default config includes all network upgrades set to block height 1, therefore all network upgrades are active by default in regtest mode.

# Usage example:
You must copy your compiled `zcashd` and `lightwalletd` binaries to `zingolib/regtest/bin/` or set up symlinks, etc. `zcash-cli` is also needed if you wish
to interact with your `zcashd` instance while it is running.

Run `cargo build` to produce the `zingo-cli` binary.

From your `zingolib/` directory, you can run:
`./target/debug/zingo-cli --regtest --data-dir regtest/data/zingo --server=127.0.0.1:9067`

This will start `zcashd` and `lightwalletd` and then connect to these tools with an interactive `zingo-cli`.
It currently takes a few seconds to do so, even on a fast machine, to give the daemons time to boot.

These daemons will be killed when the user exits `zingo-cli` using the `quit` command.
However, if there is an issue starting or shutting down regtest mode, it's possible you will have to shut down the daemons manually.

You should see several diagnostic messsages, and then:
`regtest detected and network set correctly!
Lightclient connecting to http://127.0.0.1:9067/`
at which point the interactive cli application should work with your regtest network.

`zcashd`'s stdout logfile should quickly have an output of several dozen lines, and show network upgrade activation parameters at `height=1`.
`lightwalletd`'s stdout log file will show something like:
`{"app":"lightwalletd","level":"info","msg":"Got sapling height 1 block height 1 chain regtest branchID ..."}`
...which you can view with `tail -f` or your favorite tool.

Once regtest mode is running, you can manipulate the simulated chain with `zcash-cli`.

For example, in still another terminal instance in the `zingolib/regtest/bin/` directory, you can run 
`./zcash-cli -regtest -rpcuser=xxxxxx -rpcpassword=xxxxxx generate 11` to generate 11 blocks.
Please note that by adding more than 100 blocks it is difficult or impossible to rewind the chain. The config means that after the first block all network upgrades should be in place.
Other `zcash-cli` commands should work similarly.

Invocation currently only works when being launched within a `zingolib` repo's worktree
(The paths have to know where to look for the subdirectories, they start with the top level of a `zingolib` repo, or fail immediately).

Have fun!

# Tree Diagrams
In `/zingolib/`, running `tree ./regtest`
after moving binaries and running:
./regtest/
├── bin
│   ├── lightwalletd
│   ├── zcash-cli
│   └── zcashd
├── conf
│   ├── lightwalletd.yml
│   └── zcash.conf
├── data
│   ├── lightwalletd
│   │   └── db
│   │       └── regtest
│   │           ├── blocks
│   │           └── lengths
│   ├── regtestvectors
│   │   └── regtest
│   │       ├── banlist.dat
│   │       ├── blocks
│   │       │   ├── blk00000.dat
│   │       │   ├── index
│   │       │   │   ├── 000005.ldb
│   │       │   │   ├── 000008.ldb
│   │       │   │   ├── 000009.log
│   │       │   │   ├── CURRENT
│   │       │   │   ├── LOCK
│   │       │   │   ├── LOG
│   │       │   │   ├── LOG.old
│   │       │   │   └── MANIFEST-000007
│   │       │   └── rev00000.dat
│   │       ├── chainstate
│   │       │   ├── 000005.ldb
│   │       │   ├── 000008.ldb
│   │       │   ├── 000009.log
│   │       │   ├── CURRENT
│   │       │   ├── LOCK
│   │       │   ├── LOG
│   │       │   ├── LOG.old
│   │       │   └── MANIFEST-000007
│   │       ├── database
│   │       │   └── log.0000000001
│   │       ├── db.log
│   │       ├── fee_estimates.dat
│   │       ├── peers.dat
│   │       └── wallet.dat
│   ├── zcashd
│   │   └── regtest
│   │       ├── banlist.dat
│   │       ├── blocks
│   │       │   ├── blk00000.dat
│   │       │   ├── index
│   │       │   │   ├── 000005.ldb
│   │       │   │   ├── 000008.ldb
│   │       │   │   ├── 000011.ldb
│   │       │   │   ├── 000012.log
│   │       │   │   ├── CURRENT
│   │       │   │   ├── LOCK
│   │       │   │   ├── LOG
│   │       │   │   ├── LOG.old
│   │       │   │   └── MANIFEST-000010
│   │       │   └── rev00000.dat
│   │       ├── chainstate
│   │       │   ├── 000005.ldb
│   │       │   ├── 000008.ldb
│   │       │   ├── 000011.ldb
│   │       │   ├── 000012.log
│   │       │   ├── CURRENT
│   │       │   ├── LOCK
│   │       │   ├── LOG
│   │       │   ├── LOG.old
│   │       │   └── MANIFEST-000010
│   │       ├── database
│   │       │   └── log.0000000001
│   │       ├── db.log
│   │       ├── fee_estimates.dat
│   │       ├── peers.dat
│   │       ├── wallet.dat
│   │       └── zcashd.pid
│   └── zingo
│       ├── zingo-wallet.dat
│       └── zingo-wallet.debug.log
├── logs
│   ├── lightwalletd
│   │   ├── stderr.log
│   │   └── stdout.log
│   └── zcashd
│       └── stdout.log
└── README.md

# Working Commits
Tested with `zcash` commit `d6d209`, `lightwalletd` commit `f53511c`, and `zingolib` commit `c414fc` or better.
