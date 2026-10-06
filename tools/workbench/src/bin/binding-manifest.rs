#![forbid(unsafe_code)]

use workbench::binding_manifest::{dispatch, BINARY};
use workbench::dispatch_from_root;

fn main() {
    dispatch_from_root(BINARY, dispatch)
}
