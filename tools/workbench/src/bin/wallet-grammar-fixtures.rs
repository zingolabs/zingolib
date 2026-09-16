//! Regenerate the Format Census example wallets: one synthetic Wallet File
//! per row of issue zingolabs/zingolib#2590's table, named
//! `NN_<defining-commit>.dat`, written to
//! `zingolib/src/wallet/disk/testing/grammars/` (override with `--dest`).

use std::fs;

use workbench::{parse_dest, repo_root, run, wallet_grammars};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run(
        "wallet-grammar-fixtures",
        || {
            let dest = match parse_dest(&args)? {
                Some(dir) => dir,
                None => repo_root()?.join("zingolib/src/wallet/disk/testing/grammars"),
            };
            fs::create_dir_all(&dest)
                .map_err(|e| vec![format!("cannot create {}: {e}", dest.display())])?;

            let mut lines = Vec::new();
            for fixture in wallet_grammars::all() {
                let path = dest.join(fixture.file_name());
                fs::write(&path, &fixture.bytes)
                    .map_err(|e| vec![format!("cannot write {}: {e}", path.display())])?;
                lines.push(format!(
                    "{} ({} bytes, {} line)",
                    path.display(),
                    fixture.bytes.len(),
                    fixture.branch
                ));
            }
            Ok(lines)
        },
        |lines| {
            for line in lines {
                println!("{line}");
            }
        },
    )
}
