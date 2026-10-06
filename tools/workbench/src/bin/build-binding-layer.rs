#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::iter;
use std::path;
use std::process;

use workbench::binding_layer;
use workbench::MANIFEST;

/// The program name that prefixes every diagnostic.
const PROGRAM: &str = "build-binding-layer";

/// The invocation shape, reported when the arguments do not parse.
const USAGE: &str = "usage: build-binding-layer <android|ios|kotlin> --out <directory> \
    [android only: --abi <android abi>] [android only: --in-image]";

/// The flag that runs the Android plan directly, inside a job that already runs in the builder image.
const IN_IMAGE_FLAG: &str = "--in-image";

/// The directory, in the builder image, that holds libclang.
const LIBCLANG_PATH: &str = "/usr/lib/llvm-18/lib";

/// The C flags that the builder sets for the aarch64 target.
const AARCH64_C_FLAGS: &str = "-mno-outline-atomics";

/// The number of leading command-line arguments that name the program itself.
const PROGRAM_NAME_ARGUMENTS: usize = 1;

/// The flag that names the output directory.
const OUT_FLAG: &str = "--out";

/// The flag that selects one Android ABI instead of all of them.
const ABI_FLAG: &str = "--abi";

/// The tag of the Android tool image that the builder builds and runs.
const ANDROID_IMAGE: &str = "localhost/zingolib/binding-layer-android";

/// The Dockerfile of the Android tool image, relative to the zingolib root.
const ANDROID_DOCKERFILE: &str = "bindings/android/docker/Dockerfile";

/// The build context of the Android tool image, relative to the zingolib root.
const ANDROID_CONTEXT: &str = "bindings/android/docker";

/// The directory at which the container sees the zingolib root.
const CONTAINER_ROOT: &str = "/opt/zingolib";

/// The iOS deployment target that the builder sets.
const IOS_DEPLOYMENT_TARGET: &str = "16.0";

/// The directory under the zingolib root that holds the builder's cargo output.
const BUILD_ROOT: &str = "target/binding-layer";

/// The wallet crate's directory, relative to the zingolib root.
const WALLET_CRATE_DIR: &str = "zingo-ffi/lib";

/// The directory of the workspace that holds the wallet crate and the bindgen package, which is the zingolib root.
const WALLET_WORKSPACE_DIR: &str = ".";

/// The proxy crate's directory, relative to the zingolib root.
const PROXY_CRATE_DIR: &str = "zingo-netutils/nym-proxy-ffi";

/// The wallet crate's UDL file, relative to the wallet crate's directory.
const UDL: &str = "src/zingo.udl";

/// The profile that the builder builds every library with.
const BUILDER_PROFILE: binding_layer::Profile = binding_layer::Profile::Mobile;

/// The directory under a target directory that the builder profile writes to.
const PROFILE_DIR: &str = BUILDER_PROFILE.directory();

/// The iOS output subdirectory that holds intermediate generated files.
const GENERATED_DIR: &str = "generated";

/// The iOS target that builds for devices.
const IOS_DEVICE_TARGET: &str = "aarch64-apple-ios";

/// The iOS targets that `lipo` merges into one simulator library.
const IOS_SIMULATOR_TARGETS: [&str; 2] = ["aarch64-apple-ios-sim", "x86_64-apple-ios"];

/// The directory, under a target directory, that holds merged simulator libraries.
const UNIVERSAL_SIMULATOR_DIR: &str = "universal-sim";

/// The wallet's generated C header.
const WALLET_HEADER: &str = "zingoFFI.h";

/// The wallet's generated module map.
const WALLET_MODULEMAP: &str = "zingoFFI.modulemap";

/// The proxy's generated C header.
const PROXY_HEADER: &str = "zingo_nym_proxy_ffiFFI.h";

/// The proxy's generated module map.
const PROXY_MODULEMAP: &str = "zingo_nym_proxy_ffiFFI.modulemap";

/// The module map that declares both modules inside the wallet XCFramework.
const COMBINED_MODULEMAP: &str = "module.modulemap";

/// The text between the two module maps in the combined module map.
const MODULEMAP_SEPARATOR: &str = "\n";

/// The platform whose packaging to build.
#[derive(Clone, Copy)]
enum Platform {
    /// The Android libraries and Kotlin sources for the AAR.
    Android {
        /// Whether the calling job already runs in the builder image, so the plan runs on the host.
        in_image: bool,
    },
    /// The two XCFrameworks and Swift sources for the SwiftPM package.
    Ios,
    /// The Kotlin sources alone, generated on the host for the Gradle checks that compile against them.
    Kotlin,
}

/// One action in a build plan.
enum Step {
    /// Run a command in a directory with extra environment.
    Run {
        /// The directory to run in.
        workdir: String,
        /// The environment to add.
        env: Vec<(String, String)>,
        /// The program and its arguments.
        command: Vec<String>,
    },
    /// Copy one host file to another host path, creating its parent directories.
    Copy {
        /// The source file.
        from: path::PathBuf,
        /// The destination file.
        to: path::PathBuf,
    },
    /// Write the concatenation of several host files to a host path.
    Concatenate {
        /// The files to join, in order.
        sources: Vec<path::PathBuf>,
        /// The text between consecutive files.
        separator: &'static str,
        /// The destination file.
        to: path::PathBuf,
    },
    /// Remove a host directory if it exists and create it empty.
    FreshDir(path::PathBuf),
    /// Remove a host directory if it exists.
    Remove(path::PathBuf),
}

/// Where `Run` steps execute.
enum Runner {
    /// On the host.
    Host,
    /// Inside a running container.
    Container {
        /// The container engine's program name.
        engine: &'static str,
        /// The running container's id.
        id: String,
    },
}

/// The zingolib root and the output directory, each as the host and as `Run` steps see it.
#[derive(Debug)]
struct Roots {
    /// The zingolib root as the host sees it.
    host: path::PathBuf,
    /// The zingolib root as `Run` steps see it.
    run: String,
    /// The output directory as the host sees it.
    out_host: path::PathBuf,
    /// The output directory as `Run` steps see it.
    out_run: String,
}

impl Roots {
    /// The roots of a plan whose `Run` steps execute on the host, where any output directory serves.
    fn on_host(root: path::PathBuf, out: path::PathBuf) -> Result<Self, Vec<String>> {
        refuse_an_output_directory_holding_the_root(&root, &out)?;
        Ok(Self {
            run: workbench::utf8(&root)?.to_string(),
            host: root,
            out_run: workbench::utf8(&out)?.to_string(),
            out_host: out,
        })
    }

    /// The roots of a plan whose `Run` steps execute in the container, which mounts only the zingolib root.
    fn in_container(root: path::PathBuf, out: path::PathBuf) -> Result<Self, Vec<String>> {
        refuse_an_output_directory_holding_the_root(&root, &out)?;
        // The container runs Linux, so the path under its root joins with `/`
        // whatever separator the host uses.
        let relative_out = out
            .strip_prefix(&root)
            .map_err(|_| {
                vec![format!(
                    "{} is not inside {}, which is the only directory the container mounts",
                    out.display(),
                    root.display()
                )]
            })?
            .components()
            .map(|component| component.as_os_str().to_str().map(str::to_string))
            .collect::<Option<Vec<String>>>()
            .ok_or_else(|| vec![format!("{} is not valid UTF-8", out.display())])?
            .join("/");
        let out_run = format!("{CONTAINER_ROOT}/{relative_out}");
        Ok(Self {
            host: root,
            run: CONTAINER_ROOT.to_string(),
            out_run,
            out_host: out,
        })
    }

    /// A path under the root as `Run` steps see it.
    fn run_path(&self, relative: &str) -> String {
        format!("{}/{relative}", self.run)
    }

    /// A path under the root as the host sees it.
    fn host_path(&self, relative: &str) -> path::PathBuf {
        self.host.join(relative)
    }

    /// A path under the output directory as `Run` steps see it.
    fn out_run_path(&self, relative: &str) -> String {
        format!("{}/{relative}", self.out_run)
    }

    /// A path under the output directory as the host sees it.
    fn out_host_path(&self, relative: &str) -> path::PathBuf {
        self.out_host.join(relative)
    }
}

/// Fail when the output directory, whose first step clears it, is the zingolib root or one of its ancestors.
fn refuse_an_output_directory_holding_the_root(
    root: &path::Path,
    out: &path::Path,
) -> Result<(), Vec<String>> {
    if !out.exists() {
        return Ok(());
    }
    let canonical = |directory: &path::Path| {
        directory
            .canonicalize()
            .map_err(|e| vec![format!("cannot resolve {}: {e}", directory.display())])
    };
    if canonical(root)?.starts_with(canonical(out)?) {
        Err(vec![format!(
            "{} holds the zingolib root {}, and the build clears the output directory first",
            out.display(),
            root.display()
        )])
    } else {
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(PROGRAM_NAME_ARGUMENTS).collect();
    workbench::run(
        PROGRAM,
        || build(&args),
        |out| println!("{}", out.display()),
    )
}

/// Build the selected platform's packaging and return its output directory.
fn build(args: &[String]) -> Result<path::PathBuf, Vec<String>> {
    let (platform, out, abis) = parse(args)?;
    let root = workbench::repo_root()?;
    let roots = match platform {
        Platform::Android { in_image: true } => {
            let roots = Roots::on_host(root, out)?;
            execute(&Runner::Host, &android_plan(&roots, &abis))?;
            roots
        }
        Platform::Android { in_image: false } => {
            let roots = Roots::in_container(root, out)?;
            let engine = binding_layer::container_engine()?;
            build_android_image(engine, &roots.host)?;
            let id = start_container(engine, &roots.host)?;
            let outcome = execute(
                &Runner::Container {
                    engine,
                    id: id.clone(),
                },
                &android_plan(&roots, &abis),
            );
            workbench::stdout_of(engine, &["rm", "--force", &id])?;
            outcome?;
            roots
        }
        Platform::Ios => {
            if env::consts::OS != binding_layer::MACOS {
                return Err(vec!["iOS packaging requires macOS with Xcode".to_string()]);
            }
            let roots = Roots::on_host(root, out)?;
            execute(&Runner::Host, &ios_plan(&roots))?;
            roots
        }
        Platform::Kotlin => {
            let roots = Roots::on_host(root, out)?;
            execute(&Runner::Host, &kotlin_plan(&roots))?;
            roots
        }
    };
    Ok(roots.out_host)
}

/// Parse the platform, the absolute output directory, and the selected Android ABIs.
fn parse(
    args: &[String],
) -> Result<
    (
        Platform,
        path::PathBuf,
        Vec<&'static binding_layer::AndroidAbi>,
    ),
    Vec<String>,
> {
    let (selection, flags) = args.split_first().ok_or_else(|| vec![USAGE.to_string()])?;
    let in_image = flags.iter().any(|arg| arg == IN_IMAGE_FLAG);
    let platform = match selection.as_str() {
        "android" => Platform::Android { in_image },
        "ios" | "kotlin" if in_image => {
            return Err(vec![
                format!("{IN_IMAGE_FLAG} applies only to android"),
                USAGE.to_string(),
            ])
        }
        "ios" => Platform::Ios,
        "kotlin" => Platform::Kotlin,
        other => {
            return Err(vec![
                format!("unknown platform `{other}`"),
                USAGE.to_string(),
            ])
        }
    };
    let out = workbench::flag_value(flags, OUT_FLAG)?
        .ok_or_else(|| vec![format!("missing {OUT_FLAG}"), USAGE.to_string()])?;
    let absolute_out = env::current_dir()
        .map_err(|e| vec![format!("cannot read the current directory: {e}")])?
        .join(out);
    let abis = match workbench::flag_value(flags, ABI_FLAG)? {
        None => binding_layer::ANDROID_ABIS.iter().collect(),
        Some(_) if !matches!(platform, Platform::Android { .. }) => {
            return Err(vec![
                format!("{ABI_FLAG} applies only to android"),
                USAGE.to_string(),
            ])
        }
        Some(name) => vec![binding_layer::ANDROID_ABIS
            .iter()
            .find(|abi| abi.jni_dir == name)
            .ok_or_else(|| vec![format!("unknown Android ABI `{name}`")])?],
    };
    Ok((platform, absolute_out, abis))
}

/// - Reads `rust-toolchain.toml` under `root`.
/// - Runs the container engine's `build`, which writes `ANDROID_IMAGE` to its store.
fn build_android_image(engine: &str, root: &path::Path) -> Result<(), Vec<String>> {
    let toolchain = workbench::read(&root.join(workbench::TOOLCHAIN_FILE))?;
    workbench::stdout_of(
        engine,
        &[
            "build",
            "--tag",
            ANDROID_IMAGE,
            "--build-arg",
            &image_argument(binding_layer::IMAGE_TOOLCHAIN_ARGUMENT, &toolchain),
            "--build-arg",
            &image_argument(
                binding_layer::IMAGE_TARGETS_ARGUMENT,
                &triples(binding_layer::ANDROID_ABIS.iter()).join(" "),
            ),
            "--file",
            workbench::utf8(&root.join(ANDROID_DOCKERFILE))?,
            workbench::utf8(&root.join(ANDROID_CONTEXT))?,
        ],
    )
    .map(drop)
}

fn image_argument(name: &str, value: &str) -> String {
    format!("{name}={value}")
}

fn triples<'a>(abis: impl IntoIterator<Item = &'a binding_layer::AndroidAbi>) -> Vec<&'a str> {
    abis.into_iter().map(|abi| abi.triple).collect()
}

/// Start a long-lived container of the Android tool image with zingolib mounted, and return its id.
fn start_container(engine: &str, root: &path::Path) -> Result<String, Vec<String>> {
    let mount = format!("{}:{CONTAINER_ROOT}", workbench::utf8(root)?);
    workbench::stdout_of(
        engine,
        &[
            "run",
            "--detach",
            "--volume",
            &mount,
            ANDROID_IMAGE,
            "sleep",
            "infinity",
        ],
    )
    .map(|id| id.trim().to_string())
}

fn fresh_plan(roots: &Roots, targets: &[&str], steps: Vec<Step>) -> Vec<Step> {
    let rustup = |words: &[&str]| Step::Run {
        workdir: roots.run.clone(),
        env: vec![],
        command: ["rustup"]
            .into_iter()
            .chain(words.iter().copied())
            .map(String::from)
            .collect(),
    };
    let toolchain = if targets.is_empty() {
        vec![rustup(&["toolchain", "install"])]
    } else {
        vec![
            rustup(&["toolchain", "install"]),
            rustup(&[&["target", "add"], targets].concat()),
        ]
    };
    [
        vec![Step::FreshDir(roots.out_host.clone())],
        toolchain,
        steps,
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// A `Run` step that generates one binding set into a directory, from the directory that the generation names.
fn bindgen_step(
    roots: &Roots,
    generation: binding_layer::Generation,
    language: &str,
    inputs: &binding_layer::BindgenInputs,
    out: &str,
    env: &[(String, String)],
    profile: binding_layer::Profile,
) -> Step {
    Step::Run {
        workdir: roots.run_path(workdir_dir(binding_layer::bindgen_workdir(
            generation, language,
        ))),
        env: env.to_vec(),
        command: [
            vec!["cargo".to_string()],
            binding_layer::bindgen_args(generation, language, inputs, out, profile),
        ]
        .concat(),
    }
}

/// A `Run` step that invokes cargo with the given words, then the profile's arguments, then a package selection.
fn cargo_step(
    workdir: &str,
    env: &[(String, String)],
    words: &[&str],
    profile: binding_layer::Profile,
    package: &[&str],
) -> Step {
    Step::Run {
        workdir: workdir.to_string(),
        env: env.to_vec(),
        command: [["cargo"].as_slice(), words, profile.cargo_args(), package]
            .concat()
            .into_iter()
            .map(String::from)
            .collect(),
    }
}

/// The environment that every Android step runs with.
fn android_base_env() -> Vec<(String, String)> {
    let cross = [
        ("LIBCLANG_PATH", LIBCLANG_PATH),
        ("CARGO_NDK_PLATFORM", binding_layer::ANDROID_API_LEVEL),
        (
            "CARGO_NDK_ANDROID_PLATFORM",
            binding_layer::ANDROID_API_LEVEL,
        ),
        ("AR", "llvm-ar"),
        ("LD", "ld"),
        ("RANLIB", "llvm-ranlib"),
        ("CFLAGS_aarch64_linux_android", AARCH64_C_FLAGS),
        ("CXXFLAGS_aarch64_linux_android", AARCH64_C_FLAGS),
    ]
    .map(|(key, value)| (key.to_string(), value.to_string()));
    cross.to_vec()
}

/// A plan whose every `Run` step starts from a base environment that the step's own entries override.
fn with_base_env(plan: Vec<Step>, base: &[(String, String)]) -> Vec<Step> {
    plan.into_iter()
        .map(|step| match step {
            Step::Run {
                workdir,
                env,
                command,
            } => Step::Run {
                workdir,
                env: [base.to_vec(), env].concat(),
                command,
            },
            other => other,
        })
        .collect()
}

fn android_plan(roots: &Roots, abis: &[&binding_layer::AndroidAbi]) -> Vec<Step> {
    with_base_env(
        fresh_plan(
            roots,
            &triples(abis.iter().copied()),
            android_steps(roots, abis),
        ),
        &android_base_env(),
    )
}

/// The Android steps before the base environment: bindings, per-ABI libraries, stripping, and copies.
fn android_steps(roots: &Roots, abis: &[&binding_layer::AndroidAbi]) -> Vec<Step> {
    let wallet_target = roots.run_path(&format!("{BUILD_ROOT}/android/wallet"));
    let proxy_target = roots.run_path(&format!("{BUILD_ROOT}/android/proxy"));
    let kotlin_out = roots.out_run_path(binding_layer::KOTLIN_OUT_DIR);
    let shared_library = |lib_name| {
        binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            lib_name,
            binding_layer::SHARED_SUFFIX,
        )
    };
    let wallet_file = shared_library(binding_layer::WALLET_LIB_NAME);
    let proxy_file = shared_library(binding_layer::PROXY_LIB_NAME);
    let wallet_library = |abi: &binding_layer::AndroidAbi| {
        format!("{wallet_target}/{}/{PROFILE_DIR}/{wallet_file}", abi.triple)
    };
    let proxy_library = |abi: &binding_layer::AndroidAbi| {
        format!("{proxy_target}/{}/{PROFILE_DIR}/{proxy_file}", abi.triple)
    };
    let wallet_crate_dir = roots.run_path(WALLET_CRATE_DIR);
    let proxy_crate_dir = roots.run_path(PROXY_CRATE_DIR);
    let udl = format!("{wallet_crate_dir}/{UDL}");
    let wallet_workspace = roots.run_path(&format!("{WALLET_WORKSPACE_DIR}/{MANIFEST}"));
    let bindgen = |generation, proxy_library: &str, env: &[(String, String)]| {
        bindgen_step(
            roots,
            generation,
            binding_layer::KOTLIN,
            &binding_layer::BindgenInputs {
                udl: &udl,
                wallet_workspace: &wallet_workspace,
                proxy_library,
                target_dir: &wallet_target,
            },
            &kotlin_out,
            env,
            BUILDER_PROFILE,
        )
    };
    let strip = |library: String, env: &[(String, String)]| {
        [
            vec![
                "llvm-strip".to_string(),
                "--strip-all".to_string(),
                library.clone(),
            ],
            vec![
                "llvm-objcopy".to_string(),
                "--remove-section".to_string(),
                ".comment".to_string(),
                library,
            ],
        ]
        .map(|command| Step::Run {
            workdir: wallet_crate_dir.clone(),
            env: env.to_vec(),
            command,
        })
    };
    let ndk_build =
        |abi: &binding_layer::AndroidAbi, workdir: &str, target_dir: &str, package: &[&str]| {
            cargo_step(
                workdir,
                &abi.env(target_dir),
                &["ndk", "--target", abi.triple, "build"],
                BUILDER_PROFILE,
                package,
            )
        };
    let wallet_steps = abis.iter().flat_map(|abi| {
        [ndk_build(abi, &wallet_crate_dir, &wallet_target, &[])]
            .into_iter()
            .chain(strip(wallet_library(abi), &abi.env(&wallet_target)))
    });
    let bindgen_abi = &binding_layer::ANDROID_ABIS[FIRST_POSITION];
    let proxy_steps = abis.iter().enumerate().flat_map(|(position, abi)| {
        let build = ndk_build(
            abi,
            &proxy_crate_dir,
            &proxy_target,
            &["--package", binding_layer::PROXY_PACKAGE],
        );
        let generate = (position == FIRST_POSITION).then(|| {
            bindgen(
                binding_layer::Generation::Proxy,
                &proxy_library(abi),
                &bindgen_abi.env(&wallet_target),
            )
        });
        [build]
            .into_iter()
            .chain(generate)
            .chain(strip(proxy_library(abi), &abi.env(&proxy_target)))
    });
    let host_copies = abis.iter().flat_map(|abi| {
        let jni = format!("{}/{}", binding_layer::JNI_LIBS_DIR, abi.jni_dir);
        [
            Step::Copy {
                from: host_of(roots, &wallet_library(abi)),
                to: roots
                    .out_host_path(&format!("{jni}/{}", binding_layer::ANDROID_WALLET_LIBRARY)),
            },
            Step::Copy {
                from: host_of(roots, &proxy_library(abi)),
                to: roots.out_host_path(&format!("{jni}/{proxy_file}")),
            },
        ]
    });
    [bindgen(
        binding_layer::Generation::Wallet,
        "",
        &[(
            binding_layer::TARGET_DIR_VARIABLE.to_string(),
            wallet_target.clone(),
        )],
    )]
    .into_iter()
    .chain(wallet_steps)
    .chain(proxy_steps)
    .chain(host_copies)
    .collect()
}

/// The profile of the Kotlin plan's host steps, the bindgen binaries and the proxy build, which each serve one generation.
const HOST_PROFILE: binding_layer::Profile = binding_layer::Profile::Debug;

fn kotlin_plan(roots: &Roots) -> Vec<Step> {
    let wallet_target = roots.run_path(&format!("{BUILD_ROOT}/host/wallet"));
    let proxy_target = roots.run_path(&format!("{BUILD_ROOT}/host/proxy"));
    let kotlin_out = roots.out_run_path(binding_layer::KOTLIN_OUT_DIR);
    let proxy_library = binding_layer::host_proxy_library(&proxy_target, HOST_PROFILE);
    let inputs = binding_layer::BindgenInputs {
        udl: &roots.run_path(&format!("{WALLET_CRATE_DIR}/{UDL}")),
        wallet_workspace: &roots.run_path(&format!("{WALLET_WORKSPACE_DIR}/{MANIFEST}")),
        proxy_library: &proxy_library,
        target_dir: &wallet_target,
    };
    let bindgen = |generation| {
        bindgen_step(
            roots,
            generation,
            binding_layer::KOTLIN,
            &inputs,
            &kotlin_out,
            &[],
            HOST_PROFILE,
        )
    };
    let build_proxy = cargo_step(
        &roots.run_path(PROXY_CRATE_DIR),
        &[(
            binding_layer::TARGET_DIR_VARIABLE.to_string(),
            proxy_target.clone(),
        )],
        &["build", "--locked"],
        HOST_PROFILE,
        &["--package", binding_layer::PROXY_PACKAGE],
    );
    fresh_plan(
        roots,
        &[],
        vec![
            bindgen(binding_layer::Generation::Wallet),
            build_proxy,
            bindgen(binding_layer::Generation::Proxy),
        ],
    )
}

/// The directory, relative to the zingolib root, that a bindgen working directory names.
fn workdir_dir(workdir: binding_layer::Workdir) -> &'static str {
    match workdir {
        binding_layer::Workdir::WalletCrate => WALLET_CRATE_DIR,
        binding_layer::Workdir::WalletWorkspace => WALLET_WORKSPACE_DIR,
        binding_layer::Workdir::ProxyCrate => PROXY_CRATE_DIR,
    }
}

/// The position of the first element in a sequence.
const FIRST_POSITION: usize = 0;

/// The host path of a path that a `Run` step names under the run root.
fn host_of(roots: &Roots, run_path: &str) -> path::PathBuf {
    let relative = run_path
        .strip_prefix(&roots.run)
        .map(|rest| rest.trim_start_matches('/'))
        .unwrap_or(run_path);
    roots.host_path(relative)
}

fn ios_plan(roots: &Roots) -> Vec<Step> {
    let env = vec![(
        "IPHONEOS_DEPLOYMENT_TARGET".to_string(),
        IOS_DEPLOYMENT_TARGET.to_string(),
    )];
    let wallet_target = roots.run_path(&format!("{BUILD_ROOT}/ios/wallet"));
    let proxy_target = roots.run_path(&format!("{BUILD_ROOT}/ios/proxy"));
    let with_target = |target_dir: &str| {
        [
            env.clone(),
            vec![(
                binding_layer::TARGET_DIR_VARIABLE.to_string(),
                target_dir.to_string(),
            )],
        ]
        .concat()
    };
    let wallet_crate_dir = roots.run_path(WALLET_CRATE_DIR);
    let proxy_crate_dir = roots.run_path(PROXY_CRATE_DIR);
    let wallet_workspace_dir = roots.run_path(WALLET_WORKSPACE_DIR);
    let static_library = |lib_name| {
        binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            lib_name,
            binding_layer::STATIC_SUFFIX,
        )
    };
    let wallet_static = static_library(binding_layer::WALLET_LIB_NAME);
    let proxy_static = static_library(binding_layer::PROXY_LIB_NAME);
    let library = |target_dir: &str, triple: &str, file: &str| {
        format!("{target_dir}/{triple}/{PROFILE_DIR}/{file}")
    };
    let universal =
        |target_dir: &str, file: &str| library(target_dir, UNIVERSAL_SIMULATOR_DIR, file);
    let wallet_generated = format!("{GENERATED_DIR}/wallet");
    let proxy_generated = format!("{GENERATED_DIR}/proxy");
    let headers = format!("{GENERATED_DIR}/headers");
    let udl = format!("{wallet_crate_dir}/{UDL}");
    let wallet_workspace = format!("{wallet_workspace_dir}/{MANIFEST}");
    let proxy_library = library(&proxy_target, IOS_DEVICE_TARGET, &proxy_static);
    let inputs = binding_layer::BindgenInputs {
        udl: &udl,
        wallet_workspace: &wallet_workspace,
        proxy_library: &proxy_library,
        target_dir: &wallet_target,
    };
    let bindgen = |generation, out: &str| {
        bindgen_step(
            roots,
            generation,
            binding_layer::SWIFT,
            &inputs,
            &roots.out_run_path(out),
            &env,
            BUILDER_PROFILE,
        )
    };
    let cargo_builds = |workdir: &str, target_dir: &str, package: &[&str]| {
        iter_targets()
            .map(|triple| {
                cargo_step(
                    workdir,
                    &with_target(target_dir),
                    &["build", "--target", triple],
                    BUILDER_PROFILE,
                    package,
                )
            })
            .collect::<Vec<_>>()
    };
    let lipo = |target_dir: &str, file: &str| {
        [
            Step::FreshDir(host_of(
                roots,
                &format!("{target_dir}/{UNIVERSAL_SIMULATOR_DIR}/{PROFILE_DIR}"),
            )),
            Step::Run {
                workdir: roots.run.clone(),
                env: env.clone(),
                command: ["lipo".to_string(), "-create".to_string()]
                    .into_iter()
                    .chain(
                        IOS_SIMULATOR_TARGETS
                            .iter()
                            .map(|triple| library(target_dir, triple, file)),
                    )
                    .chain(["-output".to_string(), universal(target_dir, file)])
                    .collect(),
            },
        ]
    };
    let xcframework = |name: &str, libraries: Vec<String>, with_headers: bool| {
        let output = roots.out_run_path(name);
        let header_args = if with_headers {
            vec!["-headers".to_string(), roots.out_run_path(&headers)]
        } else {
            vec![]
        };
        [
            Step::Remove(roots.out_host_path(name)),
            Step::Run {
                workdir: roots.run.clone(),
                env: env.clone(),
                command: ["xcodebuild".to_string(), "-create-xcframework".to_string()]
                    .into_iter()
                    .chain(libraries.into_iter().flat_map(|library| {
                        [vec!["-library".to_string(), library], header_args.clone()].concat()
                    }))
                    .chain(["-output".to_string(), output])
                    .collect(),
            },
        ]
    };
    let host = |relative: &str| roots.out_host_path(relative);
    let steps: Vec<Vec<Step>> = vec![
        vec![
            Step::FreshDir(host(&wallet_generated)),
            Step::FreshDir(host(&proxy_generated)),
            Step::FreshDir(host(&headers)),
            bindgen(binding_layer::Generation::Wallet, &wallet_generated),
        ],
        cargo_builds(&wallet_crate_dir, &wallet_target, &[]),
        lipo(&wallet_target, &wallet_static).into_iter().collect(),
        cargo_builds(
            &proxy_crate_dir,
            &proxy_target,
            &["-p", binding_layer::PROXY_PACKAGE],
        ),
        vec![bindgen(binding_layer::Generation::Proxy, &proxy_generated)],
        lipo(&proxy_target, &proxy_static).into_iter().collect(),
        vec![
            Step::Copy {
                from: host(&format!("{wallet_generated}/{WALLET_HEADER}")),
                to: host(&format!("{headers}/{WALLET_HEADER}")),
            },
            Step::Copy {
                from: host(&format!("{proxy_generated}/{PROXY_HEADER}")),
                to: host(&format!("{headers}/{PROXY_HEADER}")),
            },
            Step::Concatenate {
                sources: vec![
                    host(&format!("{wallet_generated}/{WALLET_MODULEMAP}")),
                    host(&format!("{proxy_generated}/{PROXY_MODULEMAP}")),
                ],
                separator: MODULEMAP_SEPARATOR,
                to: host(&format!("{headers}/{COMBINED_MODULEMAP}")),
            },
        ],
        xcframework(
            binding_layer::WALLET_XCFRAMEWORK,
            vec![
                library(&wallet_target, IOS_DEVICE_TARGET, &wallet_static),
                universal(&wallet_target, &wallet_static),
            ],
            true,
        )
        .into_iter()
        .collect(),
        xcframework(
            binding_layer::PROXY_XCFRAMEWORK,
            vec![
                library(&proxy_target, IOS_DEVICE_TARGET, &proxy_static),
                universal(&proxy_target, &proxy_static),
            ],
            false,
        )
        .into_iter()
        .collect(),
        [
            (&wallet_generated, binding_layer::WALLET_SWIFT),
            (&proxy_generated, binding_layer::PROXY_SWIFT),
        ]
        .map(|(generated, swift)| Step::Copy {
            from: host(&format!("{generated}/{swift}")),
            to: host(&format!("{}/{swift}", binding_layer::SWIFT_SOURCES_DIR)),
        })
        .into_iter()
        .collect(),
    ];
    fresh_plan(
        roots,
        &iter_targets().collect::<Vec<_>>(),
        steps.into_iter().flatten().collect(),
    )
}

/// The device target followed by the simulator targets.
fn iter_targets() -> impl Iterator<Item = &'static str> {
    [IOS_DEVICE_TARGET].into_iter().chain(IOS_SIMULATOR_TARGETS)
}

/// Execute a plan in order, stopping at the first failing step.
fn execute(runner: &Runner, plan: &[Step]) -> Result<(), Vec<String>> {
    plan.iter().try_for_each(|step| execute_step(runner, step))
}

/// Execute one step.
fn execute_step(runner: &Runner, step: &Step) -> Result<(), Vec<String>> {
    match step {
        Step::Run {
            workdir,
            env,
            command,
        } => run_command(runner, workdir, env, command),
        Step::Copy { from, to } => {
            workbench::create_parent(to)?;
            fs::copy(from, to).map(drop).map_err(|e| {
                vec![format!(
                    "cannot copy {} to {}: {e}",
                    from.display(),
                    to.display()
                )]
            })
        }
        Step::Concatenate {
            sources,
            separator,
            to,
        } => {
            let contents = sources
                .iter()
                .map(|source| workbench::read(source))
                .collect::<Result<Vec<_>, _>>()?
                .join(separator);
            workbench::create_parent(to)?;
            fs::write(to, contents).map_err(|e| vec![format!("cannot write {}: {e}", to.display())])
        }
        Step::FreshDir(directory) => workbench::fresh_dir(directory).map(drop),
        Step::Remove(directory) => {
            if directory.exists() {
                fs::remove_dir_all(directory)
                    .map_err(|e| vec![format!("cannot remove {}: {e}", directory.display())])
            } else {
                Ok(())
            }
        }
    }
}

/// A host process to start: its program, arguments, working directory, and extra environment.
struct Invocation {
    /// The program to start.
    program: String,
    /// The program's arguments.
    args: Vec<String>,
    /// The directory the host process starts in.
    workdir: String,
    /// The environment to add to the host process.
    env: Vec<(String, String)>,
}

/// The host process that runs one command on a runner.
fn invocation(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Invocation {
    match runner {
        Runner::Host => Invocation {
            program: command.first().cloned().unwrap_or_default(),
            args: command
                .iter()
                .skip(PROGRAM_NAME_ARGUMENTS)
                .cloned()
                .collect(),
            workdir: workdir.to_string(),
            env: env.to_vec(),
        },
        Runner::Container { engine, id } => Invocation {
            program: engine.to_string(),
            args: [
                "exec".to_string(),
                "--workdir".to_string(),
                workdir.to_string(),
            ]
            .into_iter()
            .chain(
                iter::once((binding_layer::TOOLCHAIN_VARIABLE.to_string(), String::new()))
                    .chain(env.iter().cloned())
                    .flat_map(|(key, value)| ["--env".to_string(), format!("{key}={value}")]),
            )
            .chain(iter::once(id.clone()))
            .chain(command.iter().cloned())
            .collect(),
            workdir: CURRENT_DIR.to_string(),
            env: vec![],
        },
    }
}

/// The working directory of a host process that only drives a container.
const CURRENT_DIR: &str = ".";

fn host_command(started: &Invocation) -> process::Command {
    let mut command = process::Command::new(&started.program);
    command
        .args(&started.args)
        .current_dir(&started.workdir)
        .env_remove(binding_layer::TOOLCHAIN_VARIABLE)
        .envs(started.env.iter().cloned());
    command
}

/// Run one command on the runner, streaming its output, and fail if it fails.
fn run_command(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<(), Vec<String>> {
    let started = invocation(runner, workdir, env, command);
    if matches!(runner, Runner::Host) && !path::Path::new(&started.workdir).is_dir() {
        return Err(vec![format!(
            "the step's working directory {} is absent",
            started.workdir
        )]);
    }
    let status = host_command(&started)
        .status()
        .map_err(|e| vec![format!("cannot run {}: {e}", started.program)])?;
    if status.success() {
        Ok(())
    } else {
        Err(vec![format!("`{}` failed ({status})", command.join(" "))])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of times one build generates the proxy bindings.
    const PROXY_GENERATIONS_PER_BUILD: usize = 1;

    const INSTALL: &str = "rustup toolchain install";

    const TARGET_ADD: &str = "rustup target add";

    const CONTAINER_ID: &str = "container-id";

    fn host_roots() -> Roots {
        Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap()
    }

    fn every_plan() -> [Vec<Step>; 3] {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        [
            android_plan(&roots(), &abis),
            ios_plan(&host_roots()),
            kotlin_plan(&host_roots()),
        ]
    }

    fn position_of(commands: &[String], leading: &str) -> Option<usize> {
        commands
            .iter()
            .position(|command| command.starts_with(leading))
    }

    #[test]
    fn every_plan_clears_its_output_then_installs_the_pin_before_its_first_cargo_step() {
        for plan in every_plan() {
            assert!(matches!(plan.first(), Some(Step::FreshDir(_))));
            let commands = commands(&plan);
            let install = commands
                .iter()
                .position(|command| command == INSTALL)
                .unwrap();
            let cargo = position_of(&commands, "cargo ").unwrap();
            assert!(install < cargo);
            assert!(commands.iter().all(|command| !command.contains("stable")));
        }
    }

    #[test]
    fn android_plan_adds_only_the_selected_targets_to_the_pinned_toolchain() {
        let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
            .iter()
            .filter(|abi| abi.jni_dir == "x86")
            .collect();
        let commands = commands(&android_plan(&roots(), &x86));
        let target_add = position_of(&commands, TARGET_ADD).unwrap();
        assert_eq!(
            commands[target_add],
            format!("{TARGET_ADD} {}", x86[FIRST_POSITION].triple)
        );
    }

    #[test]
    fn ios_plan_adds_the_device_and_simulator_targets_to_the_pinned_toolchain() {
        let commands = commands(&ios_plan(&host_roots()));
        let target_add = position_of(&commands, TARGET_ADD).unwrap();
        let targets: Vec<&str> = iter_targets().collect();
        assert_eq!(
            commands[target_add],
            format!("{TARGET_ADD} {}", targets.join(" "))
        );
    }

    #[test]
    fn kotlin_plan_adds_no_target() {
        assert!(position_of(&commands(&kotlin_plan(&host_roots())), TARGET_ADD).is_none());
    }

    #[test]
    fn no_plan_step_overrides_the_toolchain_pin() {
        for plan in every_plan() {
            assert!(plan.iter().all(|step| match step {
                Step::Run { env, .. } => env
                    .iter()
                    .all(|(key, _)| key != binding_layer::TOOLCHAIN_VARIABLE),
                _ => true,
            }));
        }
    }

    #[test]
    fn the_host_command_clears_the_toolchain_override_it_inherits() {
        let started = invocation(
            &Runner::Host,
            HOST_ROOT,
            &[("CC".to_string(), "clang".to_string())],
            &["cargo".to_string(), "build".to_string()],
        );
        let command = host_command(&started);
        let envs: Vec<_> = command.get_envs().collect();
        assert!(envs.contains(&(
            std::ffi::OsStr::new(binding_layer::TOOLCHAIN_VARIABLE),
            None
        )));
        assert!(envs.contains(&(
            std::ffi::OsStr::new("CC"),
            Some(std::ffi::OsStr::new("clang"))
        )));
    }

    #[test]
    fn the_container_command_clears_the_toolchain_override_the_image_carries() {
        let started = invocation(
            &Runner::Container {
                engine: binding_layer::ENGINES[FIRST_POSITION],
                id: CONTAINER_ID.to_string(),
            },
            CONTAINER_ROOT,
            &[("CC".to_string(), "clang".to_string())],
            &["cargo".to_string(), "build".to_string()],
        );
        let cleared = format!("{}=", binding_layer::TOOLCHAIN_VARIABLE);
        let id = started
            .args
            .iter()
            .position(|arg| arg == CONTAINER_ID)
            .unwrap();
        assert!(started.args[..id]
            .windows(2)
            .any(|pair| pair[0] == "--env" && pair[1] == cleared));
    }

    #[test]
    fn every_android_run_step_carries_the_base_environment() {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots(), &abis);
        let base = android_base_env();
        assert!(plan.iter().all(|step| match step {
            Step::Run { env, .. } => base.iter().all(|entry| env.contains(entry)),
            _ => true,
        }));
    }

    /// The zingolib root as the host sees it in tests.
    const HOST_ROOT: &str = "/host/zingolib";

    /// An output directory outside the zingolib root.
    const OUTSIDE_OUT: &str = "/host/consumer/android/build/binding-layer";

    /// An output directory inside the zingolib root.
    const INSIDE_OUT: &str = "/host/zingolib/bindings/android/build/binding-layer";

    /// The output directory that the Android workflow names, relative to the root.
    const RELATIVE_OUT: &str = "bindings/android/build/binding-layer";

    fn roots() -> Roots {
        Roots::in_container(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(INSIDE_OUT),
        )
        .unwrap()
    }

    fn commands(plan: &[Step]) -> Vec<String> {
        plan.iter()
            .filter_map(|step| match step {
                Step::Run { command, .. } => Some(command.join(" ")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn android_plan_generates_the_proxy_bindings_once_after_the_first_abi() {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots(), &abis);
        let generations = commands(&plan)
            .into_iter()
            .filter(|command| command.contains("--library"))
            .count();
        assert_eq!(generations, PROXY_GENERATIONS_PER_BUILD);
    }

    #[test]
    fn proxy_bindgen_takes_the_aarch64_environment_whatever_the_selection() {
        let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
            .iter()
            .filter(|abi| abi.jni_dir == "x86")
            .collect();
        let plan = android_plan(&roots(), &x86);
        let expected_cc = binding_layer::ANDROID_ABIS[FIRST_POSITION].cc();
        let bindgen_env = plan.iter().find_map(|step| match step {
            Step::Run { env, command, .. } if command.iter().any(|arg| arg == "--library") => {
                Some(env.clone())
            }
            _ => None,
        });
        assert!(bindgen_env
            .unwrap()
            .iter()
            .any(|(key, value)| key == "CC" && *value == expected_cc));
    }

    /// Tests that the Kotlin plan generates both binding sets after one host build of the proxy crate, and starts no NDK build.
    #[test]
    fn kotlin_plan_generates_both_sets_from_one_host_proxy_build() {
        let roots = Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap();
        let plan = kotlin_plan(&roots);
        let commands = commands(&plan);
        assert!(
            matches!(plan.first(), Some(Step::FreshDir(out)) if out == path::Path::new(OUTSIDE_OUT))
        );
        assert!(commands.iter().all(|command| !command.contains(" ndk ")));
        let wallet_generation = commands
            .iter()
            .position(|command| command.contains(UDL))
            .unwrap();
        let build = commands
            .iter()
            .position(|command| command.starts_with("cargo build"))
            .unwrap();
        let proxy_generation = commands
            .iter()
            .position(|command| command.contains("--library"))
            .unwrap();
        assert!(wallet_generation < build && build < proxy_generation);
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.contains("--language kotlin"))
                .count(),
            binding_layer::GENERATIONS.len()
        );
    }

    /// Tests that the proxy bindgen reads the library from the target directory the proxy build writes to, under the host profile.
    #[test]
    fn kotlin_proxy_bindgen_reads_the_library_the_host_build_writes() {
        let roots = Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap();
        let plan = kotlin_plan(&roots);
        let build_target = plan
            .iter()
            .find_map(|step| match step {
                Step::Run { env, command, .. } if command.join(" ").starts_with("cargo build") => {
                    env.iter()
                        .find(|(key, _)| key == binding_layer::TARGET_DIR_VARIABLE)
                        .map(|(_, target)| target.clone())
                }
                _ => None,
            })
            .unwrap();
        let library = plan
            .iter()
            .find_map(|step| match step {
                Step::Run { command, .. } => command
                    .iter()
                    .position(|arg| arg == "--library")
                    .map(|flag| command[flag + 1].clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            library,
            format!(
                "{build_target}/{}/{}{}{}",
                HOST_PROFILE.directory(),
                env::consts::DLL_PREFIX,
                binding_layer::PROXY_LIB_NAME,
                env::consts::DLL_SUFFIX
            )
        );
        assert!(commands(&plan)
            .iter()
            .all(|command| !command.contains("--release")));
    }

    /// Tests that every Kotlin generation runs from the standalone bindgen package, whose only dependency is uniffi.
    #[test]
    fn kotlin_plan_runs_every_bindgen_from_the_standalone_package() {
        let roots = Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap();
        let generations = commands(&kotlin_plan(&roots))
            .into_iter()
            .filter(|command| command.contains("--language kotlin"))
            .collect::<Vec<_>>();
        assert_eq!(generations.len(), binding_layer::GENERATIONS.len());
        assert!(generations
            .iter()
            .all(|command| command.contains("--package zingo-uniffi-bindgen")));
        assert!(generations
            .iter()
            .all(|command| !command.contains(&format!("{WALLET_CRATE_DIR}/{MANIFEST}"))));
    }

    #[test]
    fn abi_is_refused_off_android() {
        let args = |platform| {
            [platform, ABI_FLAG, "x86", OUT_FLAG, "out"]
                .map(String::from)
                .to_vec()
        };
        assert!(parse(&args("kotlin")).is_err());
        assert!(parse(&args("ios")).is_err());
        assert!(parse(&args("android")).is_ok());
    }

    #[test]
    fn android_plan_copies_both_libraries_for_every_abi() {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots(), &abis);
        let copies = plan
            .iter()
            .filter(|step| matches!(step, Step::Copy { .. }))
            .count();
        assert_eq!(
            copies,
            binding_layer::ANDROID_ABIS.len() * binding_layer::GENERATIONS.len()
        );
    }

    #[test]
    fn the_container_refuses_an_output_directory_outside_the_mounted_root() {
        let diagnostic = Roots::in_container(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap_err()
        .concat();
        assert!(diagnostic.contains(OUTSIDE_OUT) && diagnostic.contains("mounts"));
    }

    #[test]
    fn the_container_sees_the_output_directory_under_its_own_root() {
        assert_eq!(
            roots().out_run_path(binding_layer::KOTLIN_OUT_DIR),
            format!(
                "{CONTAINER_ROOT}/bindings/android/build/binding-layer/{}",
                binding_layer::KOTLIN_OUT_DIR
            )
        );
    }

    #[test]
    fn the_container_sees_the_output_directory_with_slashes_whatever_the_host_separator() {
        let root = path::PathBuf::from(HOST_ROOT);
        let out = root
            .join("bindings")
            .join("android")
            .join("build")
            .join("binding-layer");
        assert_eq!(
            Roots::in_container(root, out).unwrap().out_run,
            format!("{CONTAINER_ROOT}/{RELATIVE_OUT}")
        );
    }

    #[test]
    fn the_host_takes_an_output_directory_outside_the_root() {
        let roots = Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
        .unwrap();
        let plan = ios_plan(&roots);
        assert!(matches!(
            plan.first(),
            Some(Step::FreshDir(directory)) if directory == path::Path::new(OUTSIDE_OUT)
        ));
        assert!(commands(&plan)
            .iter()
            .any(|command| command.contains(&format!("{OUTSIDE_OUT}/{GENERATED_DIR}/wallet"))));
    }

    /// A container engine that no host has installed.
    const ABSENT_ENGINE: &str = "absent-container-engine";

    #[test]
    fn a_container_engine_that_does_not_start_is_reported_by_name_alone() {
        let runner = Runner::Container {
            engine: ABSENT_ENGINE,
            id: "container".to_string(),
        };
        let step_workdir = format!("{CONTAINER_ROOT}/{WALLET_CRATE_DIR}");
        let diagnostic = run_command(&runner, &step_workdir, &[], &["cargo".to_string()])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(ABSENT_ENGINE) && !diagnostic.contains(&step_workdir));
    }

    #[test]
    fn an_absent_host_working_directory_is_reported_before_the_program() {
        let step_workdir = format!("{HOST_ROOT}/{WALLET_CRATE_DIR}");
        let diagnostic = run_command(&Runner::Host, &step_workdir, &[], &["cargo".to_string()])
            .unwrap_err()
            .concat();
        assert!(diagnostic.contains(&step_workdir) && !diagnostic.contains("cargo"));
    }

    #[test]
    fn an_output_directory_holding_the_root_is_refused_on_both_runners() {
        let root = workbench::repo_root().unwrap();
        let parent = root.parent().unwrap().to_path_buf();
        assert!(Roots::on_host(root.clone(), root.clone()).is_err());
        assert!(Roots::on_host(root.clone(), parent.clone()).is_err());
        assert!(Roots::in_container(root.clone(), root.clone()).is_err());
        assert!(Roots::in_container(root.clone(), parent).is_err());
        assert!(Roots::on_host(root.clone(), root.join(RELATIVE_OUT)).is_ok());
    }

    #[test]
    fn host_of_maps_container_paths_back_to_the_host() {
        assert_eq!(
            host_of(&roots(), "/opt/zingolib/target/x.so"),
            path::PathBuf::from("/host/zingolib/target/x.so")
        );
    }
}
