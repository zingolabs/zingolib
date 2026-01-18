use std::io::Write;
use std::{env, fs::File, path::Path, process::Command};

fn git_description() -> Result<(), Box<dyn std::error::Error>> {
    let _fetch = Command::new("git")
        .args(["fetch", "--tags", "https://github.com/zingolabs/zingolib"])
        .output()?;

    let output = Command::new("git")
        .args(["describe", "--dirty", "--always", "--long"])
        .output()?;

    let git_description = String::from_utf8(output.stdout)?.trim_end().to_string();

    let out_dir = env::var("OUT_DIR")?;
    let dest_path = Path::new(&out_dir).join("git_description.rs");
    let mut f = File::create(dest_path)?;

    writeln!(
        f,
        "/// The result of running 'git describe' at compile time:\n\
        /// The most recent tag name, the number\n\
        /// of commits above it, and the hash of\n\
        /// the most recent commit\n\
        pub fn git_description() -> &'static str {{\"{git_description}\"}}"
    )?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn check_macos_permissions(dir: &Path) -> Result<(), String> {
    let test_file = dir.join(".write_test");
    match std::fs::write(&test_file, b"test") {
        Ok(_) => {
            let _ = std::fs::remove_file(&test_file);
            Ok(())
        }
        Err(e) => Err(format!(
            "macOS permission issue: {}\nTry: chmod -R u+rw {:?}",
            e, dir
        )),
    }
}

#[cfg(not(target_os = "macos"))]
fn check_macos_permissions(_dir: &Path) -> Result<(), String> {
    Ok(())
}

fn get_zcash_params() -> Result<(), Box<dyn std::error::Error>> {
    println!("Checking if params are available...");

    let params_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".zcash-params");

    let spend_file = params_dir.join("sapling-spend.params");
    let output_file = params_dir.join("sapling-output.params");

    // Check if params exist
    let params_exist = spend_file.exists() && output_file.exists();

    if params_exist {
        println!("✓ Found existing params at {:?}", params_dir);

        // On macOS, check if we have write permissions before calling download_sapling_parameters
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = check_macos_permissions(&params_dir) {
                println!("Warning: No write access to params dir, but params exist");
                println!("  Attempting to use existing params without integrity check...");
                println!("  {}", e);

                // Try to make params readable at minimum
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&spend_file)?.permissions();
                perms.set_mode(0o444); // Read-only
                std::fs::set_permissions(&spend_file, perms.clone()).ok();
                std::fs::set_permissions(&output_file, perms).ok();
            }
        }
    }

    // Always call download_sapling_parameters - it will:
    // 1. Check if params exist
    // 2. Verify their integrity (SHA256 checksums)
    // 3. Download if missing or corrupted
    // 4. Return paths to valid params
    let params_path = zcash_proofs::download_sapling_parameters(Some(400))
        .map_err(|e| {
            let err_msg = format!("Failed to download/verify params: {}", e);
            // If params exist but we got permission error, provide helpful message
            if params_exist && e.to_string().contains("Permission denied") {
                format!(
                    "{}\n\nThe params exist but cannot be verified due to permissions.\n\
                    This is a known issue on macOS. Options:\n\
                    1. Fix permissions: sudo chown -R $USER ~/.zcash-params && chmod -R u+rw ~/.zcash-params\n\
                    2. Remove and re-download: rm -rf ~/.zcash-params\n\
                    3. Open an issue at: https://github.com/zingolabs/zingolib/issues",
                    err_msg
                )
            } else {
                err_msg
            }
        })?;

    println!("✓ Params verified");
    println!("  Spend: {:?}", params_path.spend);
    println!("  Output: {:?}", params_path.output);

    // Copy to internal location
    let internal_params_path = Path::new("zcash-params");
    std::fs::create_dir_all(internal_params_path)
        .map_err(|e| format!("Cannot create {:?}: {}", internal_params_path, e))?;

    std::fs::copy(
        &params_path.spend,
        internal_params_path.join("sapling-spend.params"),
    )
    .map_err(|e| format!("Cannot copy spend params: {}", e))?;

    std::fs::copy(
        &params_path.output,
        internal_params_path.join("sapling-output.params"),
    )
    .map_err(|e| format!("Cannot copy output params: {}", e))?;

    println!("✓ Params copied to internal location");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("\n=== Zingolib Build Script ===\n");

    match get_zcash_params() {
        Ok(_) => println!("✓ Zcash params setup complete"),
        Err(e) => {
            eprintln!("\n✗ Failed to setup Zcash params:\n  {}", e);
            eprintln!("\nTroubleshooting:");
            eprintln!("  1. Check: ls -la ~/.zcash-params/");
            eprintln!("  2. Fix: chmod -R u+rw ~/.zcash-params/");
            eprintln!("  3. Reset: rm -rf ~/.zcash-params/");
            return Err(e);
        }
    }

    match git_description() {
        Ok(_) => println!("✓ Git description generated"),
        Err(e) => {
            eprintln!("Warning: Failed to generate git description: {}", e);
        }
    }

    println!("\n✓ Build script completed successfully\n");
    Ok(())
}
