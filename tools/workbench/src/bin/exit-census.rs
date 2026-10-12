//! Census of the Exit Nodes the Nym directory advertises, reported by
//! gateway rather than by exit.
//!
//! An `ExitNodeId` is a network requester's Nym address in the Recipient
//! form `<client_id>.<client_enc>@<gateway_id>`, so the gateway a given
//! exit egresses through is a substring of its identity. Whether two exits
//! ever share a gateway decides whether independent draws from the Exit
//! Pool are independent *failure domains*: exits sharing a gateway fail
//! together when that gateway does, and a quartet drawing four exits from
//! three gateways has only three independent chances.
//!
//! This asks the question with one discovery call and no births.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::process::Command;

use workbench::{repo_root, run};

/// The stdout line prefix the proxy's discover mode announces each Exit
/// Node under. An agreeing representation of `zingo_netutils::
/// NYM_EXIT_LINE_PREFIX`, restated because this crate is deliberately
/// std-only and takes no production dependency; the two must not drift.
const NYM_EXIT_LINE_PREFIX: &str = "NYM_EXIT=";

/// The separator between a network requester and the gateway it registered
/// at, within one Recipient address.
const GATEWAY_SEPARATOR: char = '@';

fn main() {
    run("exit-census", census, |()| {})
}

fn census() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    let proxy = root.join("target").join("debug").join("nym-proxy");
    if !proxy.exists() {
        return Err(vec![format!(
            "no nym-proxy at {}: build it with `makers bundle-nym-proxy`",
            proxy.display()
        )]);
    }

    let output = Command::new(&proxy)
        .arg("--discover")
        .output()
        .map_err(|e| vec![format!("cannot run {}: {e}", proxy.display())])?;
    if !output.status.success() {
        return Err(vec![
            format!("discovery failed ({})", output.status),
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ]);
    }

    let exits: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix(NYM_EXIT_LINE_PREFIX))
        .map(str::to_string)
        .collect();
    if exits.is_empty() {
        return Err(vec!["discovery announced no exits".to_string()]);
    }

    // Group by the gateway each exit names, and separately by the requester
    // key, so a requester registered at two gateways is visible as one key
    // under two addresses.
    let mut by_gateway: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut by_requester: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut malformed = 0usize;
    for exit in &exits {
        match exit.split_once(GATEWAY_SEPARATOR) {
            Some((requester, gateway)) => {
                by_gateway.entry(gateway).or_default().push(requester);
                by_requester.entry(requester).or_default().push(gateway);
            }
            None => malformed += 1,
        }
    }

    let shared: Vec<_> = by_gateway.iter().filter(|(_, r)| r.len() > 1).collect();
    let republished: Vec<_> = by_requester.iter().filter(|(_, g)| g.len() > 1).collect();

    let mut spread: BTreeMap<usize, usize> = BTreeMap::new();
    for requesters in by_gateway.values() {
        *spread.entry(requesters.len()).or_default() += 1;
    }

    println!("exits advertised:        {}", exits.len());
    println!("distinct gateways:       {}", by_gateway.len());
    println!("distinct requesters:     {}", by_requester.len());
    println!("addresses without '{GATEWAY_SEPARATOR}':  {malformed}");
    println!("gateways hosting >1 exit: {}", shared.len());
    println!("requesters at >1 gateway: {}", republished.len());
    println!("exits per gateway:");
    for (exits_here, gateways) in &spread {
        println!("  {exits_here} exit(s): {gateways} gateway(s)");
    }
    if by_gateway.len() == exits.len() {
        println!("\nevery exit is its own gateway: draws are independent failure domains");
    } else {
        println!(
            "\n{} exits share {} gateways: draws are NOT fully independent",
            exits.len(),
            by_gateway.len()
        );
    }
    Ok(())
}
