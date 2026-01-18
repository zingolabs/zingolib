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

fn is_ci_environment() -> bool {
    std::env::var("CI").is_ok()
        || std::env::var("GITHUB_ACTIONS").is_ok()
        || std::env::var("GITLAB_CI").is_ok()
}

fn get_zcash_params() -> Result<(), Box<dyn std::error::Error>> {
    println!("Checking if params are available...");

    // Skip params setup in CI environments where tests don't need actual params
    if is_ci_environment() {
        println!("CI environment detected - skipping params download");
        println!("Note: Integration tests requiring params will be skipped");

        // Create empty params directory for build to succeed
        let internal_params_path = Path::new("zcash-params");
        std::fs::create_dir_all(internal_params_path).ok();

        return Ok(());
    }

    let params_dir = dirs::home_dir()
        .ok_or("Cannot determine home directory")?
        .join(".zcash-params");

    if params_dir.exists() {
        println!("Params directory exists at {:?}", params_dir);

        // Check macOS permissions
        if let Err(e) = check_macos_permissions(&params_dir) {
            eprintln!("Warning: {}", e);
        }

        std::fs::read_dir(&params_dir)
            .map_err(|e| format!("Cannot access {:?}: {}", params_dir, e))?;
        println!("✓ Params directory is accessible");
    }

    let params_path = zcash_proofs::download_sapling_parameters(Some(400))
        .map_err(|e| format!("Failed to download/access params: {}", e))?;

    println!("✓ Params available");
    println!("  Spend: {:?}", params_path.spend);
    println!("  Output: {:?}", params_path.output);

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
