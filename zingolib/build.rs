use std::io::Write;
use std::{env, fs::File, path::Path, process::Command};

fn git_description() {
    let _fetch = Command::new("git")
        .args(["fetch", "--tags", "https://github.com/zingolabs/zingolib"])
        .output()
        .expect("Failed to execute git command");
    let output = Command::new("git")
        .args(["describe", "--dirty", "--always", "--long"])
        .output()
        .expect("Failed to execute git command");

    eprintln!("Git command output: {output:?}");
    println!("Git command output: {output:?}");

    let git_description = String::from_utf8(output.stdout)
        .unwrap()
        .trim_end()
        .to_string();

    // Write the git description to a file which will be included in the crate
    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path).unwrap();
    writeln!(
        f,
        "/// The result of running 'git describe' at compile time:\n\
        /// The most recent tag name, the number\n\
        /// of commits above it, and the hash of\n\
        /// the most recent commit\n\
        pub fn git_description() -> &'static str {{\"{git_description}\"}}"
    )
    .unwrap();
}

/// Checks if zcash params are available and downloads them if not.
/// Also copies them to an internal location for use by mobile platforms.
fn get_zcash_params() {
    println!("Checking if params are available...");

    // First check if params already exist locally
    let internal_params_path = Path::new("zcash-params");
    let spend_path = internal_params_path.join("sapling-spend.params");
    let output_path = internal_params_path.join("sapling-output.params");

    if spend_path.exists() && output_path.exists() {
        println!("Params already exist locally, skipping download");
        return;
    }

    // Try to download params
    let params_path = match zcash_proofs::download_sapling_parameters(Some(400)) {
        Ok(p) => {
            println!("Params downloaded!");
            println!("Spend path: {}", p.spend.to_str().unwrap());
            println!("Output path: {}", p.output.to_str().unwrap());
            p
        }
        Err(e) => {
            println!("Warning: Could not download params: {e}");
            println!("Checking if params exist in default location...");

            // Try to find params in ~/.zcash-params or ZCASH_PARAMS_DIR
            let params_dir = std::env::var("ZCASH_PARAMS_DIR")
                .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.zcash-params", h)))
                .unwrap_or_else(|_| ".zcash-params".to_string());

            let default_spend = Path::new(&params_dir).join("sapling-spend.params");
            let default_output = Path::new(&params_dir).join("sapling-output.params");

            if !default_spend.exists() || !default_output.exists() {
                eprintln!(
                    "ERROR: Could not download params and they don't exist in {}",
                    params_dir
                );
                eprintln!("Please manually download from https://download.z.cash/downloads/");
                panic!("Missing Zcash parameters");
            }

            println!("Found params in {}", params_dir);

            // Return the same type that download_sapling_parameters returns
            zcash_proofs::SaplingParameterPaths {
                spend: default_spend,
                output: default_output,
            }
        }
    };

    // Copy the params to the internal location
    if let Err(e) = std::fs::create_dir_all(internal_params_path) {
        eprintln!("Warning: Could not create zcash-params directory: {e}");
        return;
    }

    if let Err(e) = std::fs::copy(&params_path.spend, &spend_path) {
        eprintln!("Warning: Could not copy spend params: {e}");
    } else {
        println!("Copied spend params to local directory");
    }

    if let Err(e) = std::fs::copy(&params_path.output, &output_path) {
        eprintln!("Warning: Could not copy output params: {e}");
    } else {
        println!("Copied output params to local directory");
    }

    println!("Params setup complete");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    get_zcash_params();
    git_description();
    Ok(())
}
