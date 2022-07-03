before:
regtest/
├── bin
├── confs
│   ├── lightwalletdconf.yml
│   └── zcash.conf
├── datadir
│   ├── lightwalletd
│   ├── zcash
│   └── zingo
├── logs
└── README.md

after:
tree regtest/
regtest/
├── bin
│   ├── lightwalletd
│   ├── zcash-cli
│   └── zcashd
├── confs
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
