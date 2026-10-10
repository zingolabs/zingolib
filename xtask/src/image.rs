use std::path::Path;

use crate::ci_plan::{self, ARTIFACTS_ENV_FILE, IMAGE_DIR};
use crate::container::Runtime;
use crate::{run_streaming_in, toolchain_channel};

const USAGE: &str = "usage: cargo xtask image <build | ensure>";

const DOCKERFILE: &str = "docker-ci/Dockerfile";

const PLATFORM: &str = "--platform=linux/amd64";

const RUST_VERSION_ARG: &str = "RUST_VERSION";

const ENV_BUILD_ARGS: [&str; 5] = [
    "ZAINO_IMAGE_TAG",
    "ZAINO_IMAGE_DIGEST",
    "ZEBRA_VERSION",
    "ZEBRA_IMAGE_DIGEST",
    "NEXTEST_VERSION",
];

/// - Builds or probes the test image through the container runtime.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["build"] => build(root, &Runtime::detect()?),
        ["ensure"] => ensure(root),
        _ => Err(vec![USAGE.to_string()]),
    }
}

/// - Runs `<runtime> image inspect` as a child process in `root`.
/// - Builds the image when the runtime lacks it.
pub fn ensure(root: &Path) -> Result<(), Vec<String>> {
    let runtime = Runtime::detect()?;
    runtime.ensure_volumes(root)?;
    let image = ci_plan::image(root)?;
    println!("Checking for {image} with {}...", runtime.program());
    if runtime.has_image(root, &image)? {
        println!("Image {image} already exists.");
        Ok(())
    } else {
        println!("Image {image} not found; building it now.");
        build(root, &runtime)
    }
}

/// - Runs `<runtime> volume inspect` and `<runtime> volume create` as child processes.
/// - Runs `<runtime> build` as a child process in `root`, with every stream inherited.
pub fn build(root: &Path, runtime: &Runtime) -> Result<(), Vec<String>> {
    runtime.ensure_volumes(root)?;
    let image = ci_plan::image(root)?;
    let env = ci_plan::artifacts_env(root)?;
    let mut build_args = vec![format!("{RUST_VERSION_ARG}={}", toolchain_channel(root)?)];
    for key in ENV_BUILD_ARGS {
        build_args.push(format!("{key}={}", ci_plan::env_value(&env, key)?));
    }

    println!("Building {image} from {ARTIFACTS_ENV_FILE}");
    for build_arg in &build_args {
        println!("  {build_arg}");
    }

    let mut args = vec![
        "build".to_string(),
        PLATFORM.to_string(),
        "-f".to_string(),
        DOCKERFILE.to_string(),
    ];
    for build_arg in &build_args {
        args.push("--build-arg".to_string());
        args.push(build_arg.clone());
    }
    args.extend(["-t".to_string(), image, IMAGE_DIR.to_string()]);
    run_streaming_in(
        root,
        runtime.program(),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        &[],
    )
}
