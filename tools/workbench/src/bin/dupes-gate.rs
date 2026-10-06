#![forbid(unsafe_code)]

use workbench::dispatch_from_root;
use workbench::dupes_gate::{dispatch, BINARY};

fn main() {
    dispatch_from_root(BINARY, dispatch)
}
