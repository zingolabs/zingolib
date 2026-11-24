#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

const LOCK: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"));
const TRANSPORT_CRATE: &str = env!("CARGO_PKG_NAME");
const WALLET_STACK_CRATES: [&str; 6] = [
    "zl-common",
    "zcash_primitives",
    "zcash_protocol",
    "zcash_keys",
    "orchard",
    "sapling-crypto",
];

const PACKAGE_HEADER: &str = "[[package]]";
const NAME_PREFIX: &str = "name = \"";
const DEPENDENCIES_HEADER: &str = "dependencies = [";
const LIST_END: &str = "]";
const QUOTE: char = '"';
const VERSION_SEPARATOR: char = ' ';

fn lock_edges(lock: &str) -> BTreeMap<&str, BTreeSet<&str>> {
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut package = None;
    let mut in_dependencies = false;
    for line in lock.lines() {
        if line == PACKAGE_HEADER {
            package = None;
            in_dependencies = false;
        } else if let Some(name) = line.strip_prefix(NAME_PREFIX) {
            package = name.split(QUOTE).next();
            if let Some(package) = package {
                edges.entry(package).or_default();
            }
        } else if line == DEPENDENCIES_HEADER {
            in_dependencies = true;
        } else if line == LIST_END {
            in_dependencies = false;
        } else if in_dependencies
            && let Some(package) = package
            && let Some(dependency) = line
                .trim()
                .trim_start_matches(QUOTE)
                .split([VERSION_SEPARATOR, QUOTE])
                .next()
        {
            edges.entry(package).or_default().insert(dependency);
        }
    }

    edges
}

fn reached<'lock>(
    edges: &BTreeMap<&'lock str, BTreeSet<&'lock str>>,
    start: &'lock str,
) -> BTreeSet<&'lock str> {
    let mut reached = BTreeSet::new();
    let mut frontier = vec![start];
    while let Some(package) = frontier.pop() {
        if reached.insert(package) {
            frontier.extend(edges.get(package).into_iter().flatten());
        }
    }

    reached
}

#[test]
fn the_transport_crate_reaches_no_wallet_stack_crate() {
    let edges = lock_edges(LOCK);
    assert!(
        edges.contains_key(TRANSPORT_CRATE),
        "the lock file lists no package named {TRANSPORT_CRATE}"
    );

    let reached = reached(&edges, TRANSPORT_CRATE);
    let wallet_stack = WALLET_STACK_CRATES
        .into_iter()
        .filter(|wallet_stack| reached.contains(wallet_stack))
        .collect::<Vec<_>>();

    assert!(
        wallet_stack.is_empty(),
        "{TRANSPORT_CRATE} reaches {wallet_stack:?} among its {} packages",
        reached.len()
    );
}
