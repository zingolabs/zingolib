Integration tests use the `regtest` mod to manage (previously built) lightwalletd and zcashd instances.  The zcashd
instance runs in `regtest mode` meaning that it's not attached to an external blockchain, and can generate blocks
on its own self-validated blockchain.

Test authors that want an out-of-the box scenario can select from the existing set.

The entrypoint is:

  0. cargo doc --document-private-items
  1. point a browser at:
      file:///$REPOSITORY_ROOT/target/doc/zingo_cli/index.html