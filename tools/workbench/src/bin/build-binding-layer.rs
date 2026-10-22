#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::io;
use std::io::BufRead;
use std::iter;
use std::path;
use std::process;

use workbench::binding_layer;
use workbench::binding_manifest;
use workbench::MANIFEST;

/// The program name that prefixes every diagnostic.
const PROGRAM: &str = "build-binding-layer";

/// The invocation shape, reported when the arguments do not parse.
const USAGE: &str = "usage: build-binding-layer <android|kotlin> --out <directory> \
    [android only: --abi <android abi>] [android only: --in-image] \
    | build-binding-layer ios [--target <ios triple> | --assemble] \
    | build-binding-layer artifact --commit <commit> \
    | build-binding-layer image";

const ARTIFACT_COMMAND: &str = "artifact";

const TRIPLE_LIST_SEPARATOR: &str = ", ";

const IMAGE_COMMAND: &str = "image";

const COMMIT_FLAG: &str = "--commit";

const REVISION_PATH_SEPARATOR: &str = ":";

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

const TARGET_FLAG: &str = "--target";

const ASSEMBLE_FLAG: &str = "--assemble";

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

/// The directory of the workspace that holds the wallet crate and the bindgen package, which is the zingolib root.
const WALLET_WORKSPACE_DIR: &str = ".";

const BUILDER_PROFILE: binding_layer::Profile = binding_layer::Profile::Mobile;

const PROFILE_DIR: &str = BUILDER_PROFILE.directory();

/// The iOS output subdirectory that holds intermediate generated files.
const GENERATED_DIR: &str = "generated";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IosArch {
    Arm64,
    X86_64,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IosTarget {
    Device,
    Simulator(IosArch),
}

impl IosTarget {
    const fn triple(self) -> &'static str {
        match self {
            IosTarget::Device => "aarch64-apple-ios",
            IosTarget::Simulator(IosArch::Arm64) => "aarch64-apple-ios-sim",
            IosTarget::Simulator(IosArch::X86_64) => "x86_64-apple-ios",
        }
    }

    const fn arch(self) -> &'static str {
        match self {
            IosTarget::Device | IosTarget::Simulator(IosArch::Arm64) => "arm64",
            IosTarget::Simulator(IosArch::X86_64) => "x86_64",
        }
    }

    const fn platform(self) -> &'static str {
        match self {
            IosTarget::Device => "ios",
            IosTarget::Simulator(_) => "ios-simulator",
        }
    }

    fn from_triple(triple: &str) -> Option<IosTarget> {
        iter_ios_targets().find(|target| target.triple() == triple)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IosPhase {
    Full,
    Slice(IosTarget),
    Assembly,
}

const IOS_SIMULATORS: [IosTarget; 2] = [
    IosTarget::Simulator(IosArch::Arm64),
    IosTarget::Simulator(IosArch::X86_64),
];

/// The directory, under a target directory, that holds merged simulator libraries.
const UNIVERSAL_SIMULATOR_DIR: &str = "universal-sim";

/// The directory, under a profile directory, that holds each hidden-symbol archive and its merged object.
const HIDDEN_DIR: &str = "hidden";

const SLICES_DIR: &str = "slices";

const OBJECT_SUFFIX: &str = ".o";

const LD: &str = "ld";

const RELOCATABLE_FLAG: &str = "-r";

const ARCH_FLAG: &str = "-arch";

const PLATFORM_VERSION_FLAG: &str = "-platform_version";

const FORCE_LOAD_FLAG: &str = "-force_load";

const EXPORTED_SYMBOL_FLAG: &str = "-exported_symbol";

const OUTPUT_FLAG: &str = "-o";

const LIBTOOL: &str = "libtool";

const LIBTOOL_STATIC_FLAG: &str = "-static";

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

const SIMCTL_LIST_AVAILABLE_DEVICES: [&str; 5] =
    ["xcrun", "simctl", "list", "devices", "available"];

const XCODEBUILD_TEST: [&str; 2] = ["xcodebuild", "test"];

const SCHEME_FLAG: &str = "-scheme";

const DESTINATION_FLAG: &str = "-destination";

const SIMULATOR_DESTINATION: &str = "platform=iOS Simulator,id=";
const SIMULATOR_ARCH: &str = ",arch=arm64";

fn xcodebuild_test(scheme: &str, udid: &str) -> Vec<String> {
    XCODEBUILD_TEST
        .into_iter()
        .chain([SCHEME_FLAG, scheme, DESTINATION_FLAG])
        .map(String::from)
        .chain(iter::once(format!(
            "{SIMULATOR_DESTINATION}{udid}{SIMULATOR_ARCH}"
        )))
        .collect()
}

/// The platform whose packaging to build.
#[derive(Clone, Copy)]
enum Platform {
    /// The Android libraries and Kotlin sources for the AAR.
    Android {
        /// Whether the calling job already runs in the builder image, so the plan runs on the host.
        in_image: bool,
    },
    Ios(IosPhase),
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
    Require(path::PathBuf),
    Describe {
        workdir: String,
        env: Vec<(String, String)>,
        ndk: Option<binding_layer::NdkLink>,
        command: Vec<String>,
        to: path::PathBuf,
    },
    XcodeTest {
        workdir: String,
        scheme: String,
    },
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
    if workbench::canonical(root)?.starts_with(workbench::canonical(out)?) {
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
    match args.split_first() {
        Some((command, rest)) if command == ARTIFACT_COMMAND => {
            workbench::run(PROGRAM, || artifact(rest), |outputs| print!("{outputs}"))
        }
        Some((command, rest)) if command == IMAGE_COMMAND => {
            workbench::run(PROGRAM, || image(rest), |tag| println!("{tag}"))
        }
        _ => workbench::run(
            PROGRAM,
            || build(&args),
            |out| println!("{}", out.display()),
        ),
    }
}

/// - Runs `git show` to read the wallet manifest at the commit.
fn artifact(args: &[String]) -> Result<String, Vec<String>> {
    let commit = workbench::required_flag(args, COMMIT_FLAG, USAGE)?;
    let manifest = workbench::git(&[
        "show",
        &[commit, binding_layer::WALLET_MANIFEST].join(REVISION_PATH_SEPARATOR),
    ])?;
    let crate_version = binding_layer::crate_version(&manifest)?;
    Ok(binding_layer::render_outputs(
        &binding_layer::artifact_outputs(&binding_manifest::PLATFORMS, commit, &crate_version)?,
    ))
}

/// - Runs the container engine's `build`, which writes `PUBLISHED_ANDROID_IMAGE` to its store.
fn image(args: &[String]) -> Result<&'static str, Vec<String>> {
    if let Some(extra) = args.first() {
        return Err(vec![
            format!("unexpected argument `{extra}`"),
            USAGE.to_string(),
        ]);
    }
    let engine = binding_layer::container_engine()?;
    build_android_image(
        engine,
        &workbench::repo_root()?,
        binding_layer::PUBLISHED_ANDROID_IMAGE,
    )?;
    Ok(binding_layer::PUBLISHED_ANDROID_IMAGE)
}

/// Build the selected platform's packaging and return its output directory.
fn build(args: &[String]) -> Result<path::PathBuf, Vec<String>> {
    let Arguments {
        platform,
        out,
        abis,
    } = parse(args)?;
    let root = workbench::repo_root()?;
    let roots = match platform {
        Platform::Android { in_image: true } => {
            let roots = Roots::on_host(root, required_out(out)?)?;
            execute(&Runner::Host, &android_plan(&roots, &abis))?;
            roots
        }
        Platform::Android { in_image: false } => {
            let roots = Roots::in_container(root, required_out(out)?)?;
            let engine = binding_layer::container_engine()?;
            build_android_image(engine, &roots.host, ANDROID_IMAGE)?;
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
        Platform::Ios(phase) => {
            if env::consts::OS != binding_layer::MACOS {
                return Err(vec!["iOS packaging requires macOS with Xcode".to_string()]);
            }
            let out = root.join(binding_layer::swift_package_output_dir());
            let roots = Roots::on_host(root, out)?;
            execute(&Runner::Host, &ios_plan(&roots, phase))?;
            roots
        }
        Platform::Kotlin => {
            let roots = Roots::on_host(root, required_out(out)?)?;
            execute(&Runner::Host, &kotlin_plan(&roots))?;
            roots
        }
    };
    Ok(roots.out_host)
}

fn required_out(out: Option<path::PathBuf>) -> Result<path::PathBuf, Vec<String>> {
    out.ok_or_else(|| vec![format!("missing {OUT_FLAG}"), USAGE.to_string()])
}

struct Arguments {
    platform: Platform,
    out: Option<path::PathBuf>,
    abis: Vec<&'static binding_layer::AndroidAbi>,
}

fn parse(args: &[String]) -> Result<Arguments, Vec<String>> {
    let (selection, flags) = args.split_first().ok_or_else(|| vec![USAGE.to_string()])?;
    let in_image = flags.iter().any(|arg| arg == IN_IMAGE_FLAG);
    let target = workbench::flag_value(flags, TARGET_FLAG)?;
    let assemble = flags.iter().any(|arg| arg == ASSEMBLE_FLAG);
    let platform = match selection.as_str() {
        "android" => Platform::Android { in_image },
        "ios" | "kotlin" if in_image => {
            return Err(vec![
                format!("{IN_IMAGE_FLAG} applies only to android"),
                USAGE.to_string(),
            ])
        }
        "ios" => Platform::Ios(ios_phase(target, assemble)?),
        "kotlin" => Platform::Kotlin,
        other => {
            return Err(vec![
                format!("unknown platform `{other}`"),
                USAGE.to_string(),
            ])
        }
    };
    if (target.is_some() || assemble) && !matches!(platform, Platform::Ios(_)) {
        return Err(vec![
            format!("{TARGET_FLAG} and {ASSEMBLE_FLAG} apply only to ios"),
            USAGE.to_string(),
        ]);
    }
    let out = workbench::flag_value(flags, OUT_FLAG)?;
    if out.is_some() && matches!(platform, Platform::Ios(_)) {
        return Err(vec![
            format!("{OUT_FLAG} applies only to android and kotlin"),
            USAGE.to_string(),
        ]);
    }
    let absolute_out = match out {
        None => None,
        Some(out) => Some(
            env::current_dir()
                .map_err(|e| vec![format!("cannot read the current directory: {e}")])?
                .join(out),
        ),
    };
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
    Ok(Arguments {
        platform,
        out: absolute_out,
        abis,
    })
}

fn ios_phase(target: Option<&str>, assemble: bool) -> Result<IosPhase, Vec<String>> {
    match (target, assemble) {
        (None, false) => Ok(IosPhase::Full),
        (None, true) => Ok(IosPhase::Assembly),
        (Some(_), true) => Err(vec![
            format!("{TARGET_FLAG} and {ASSEMBLE_FLAG} exclude each other"),
            USAGE.to_string(),
        ]),
        (Some(triple), false) => IosTarget::from_triple(triple)
            .map(IosPhase::Slice)
            .ok_or_else(|| {
                vec![
                    format!(
                        "unknown iOS target `{triple}`; the targets are {}",
                        iter_ios_targets()
                            .map(|target| target.triple())
                            .collect::<Vec<_>>()
                            .join(TRIPLE_LIST_SEPARATOR)
                    ),
                    USAGE.to_string(),
                ]
            }),
    }
}

/// - Reads `rust-toolchain.toml` under `root`.
/// - Runs the container engine's `build`, which writes `tag` to its store.
fn build_android_image(engine: &str, root: &path::Path, tag: &str) -> Result<(), Vec<String>> {
    let toolchain = workbench::read(&root.join(workbench::TOOLCHAIN_FILE))?;
    let targets = triples(binding_layer::ANDROID_ABIS.iter()).join(" ");
    workbench::stdout_of(
        engine,
        &[
            "build",
            "--tag",
            tag,
            "--build-arg",
            &format!("{}={toolchain}", binding_layer::IMAGE_TOOLCHAIN_ARGUMENT),
            "--build-arg",
            &format!("{}={targets}", binding_layer::IMAGE_TARGETS_ARGUMENT),
            "--file",
            workbench::utf8(&root.join(ANDROID_DOCKERFILE))?,
            workbench::utf8(&root.join(ANDROID_CONTEXT))?,
        ],
    )
    .map(drop)
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

fn toolchain_plan(roots: &Roots, targets: &[&str], steps: Vec<Step>) -> Vec<Step> {
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
    [toolchain, steps].into_iter().flatten().collect()
}

fn fresh_plan(roots: &Roots, targets: &[&str], steps: Vec<Step>) -> Vec<Step> {
    [
        vec![Step::FreshDir(roots.out_host.clone())],
        toolchain_plan(roots, targets, steps),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn bindgen_step(
    roots: &Roots,
    generation: binding_layer::Generation,
    inputs: &binding_layer::BindgenInputs,
    library: &str,
    out: &str,
    env: &[(String, String)],
) -> Step {
    Step::Run {
        workdir: roots.run_path(generation.names().crate_dir),
        env: env.to_vec(),
        command: [
            vec!["cargo".to_string()],
            binding_layer::bindgen_args(inputs, library, out),
        ]
        .concat(),
    }
}

fn cargo_step(
    workdir: &str,
    env: &[(String, String)],
    words: &[&str],
    profile: binding_layer::Profile,
    selection: &[&str],
) -> Step {
    Step::Run {
        workdir: workdir.to_string(),
        env: env.to_vec(),
        command: cargo_command(words, profile, selection),
    }
}

fn cargo_command(
    words: &[&str],
    profile: binding_layer::Profile,
    selection: &[&str],
) -> Vec<String> {
    [["cargo"].as_slice(), words, profile.cargo_args(), selection]
        .concat()
        .into_iter()
        .map(String::from)
        .collect()
}

fn describe_step(
    workdir: &str,
    env: &[(String, String)],
    ndk: Option<binding_layer::NdkLink>,
    target: &str,
    profile: binding_layer::Profile,
    selection: &[&str],
    to: path::PathBuf,
) -> Step {
    let mut command = cargo_command(&["build", "--target", target], profile, selection);
    command.push(binding_layer::MESSAGE_FORMAT_FLAG.to_string());
    Step::Describe {
        workdir: workdir.to_string(),
        env: env.to_vec(),
        ndk,
        command,
        to,
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
            Step::Describe {
                workdir,
                env,
                ndk,
                command,
                to,
            } => Step::Describe {
                workdir,
                env: [base.to_vec(), env].concat(),
                ndk,
                command,
                to,
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
    let target = roots.run_path(&format!("{BUILD_ROOT}/android"));
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
        format!("{target}/{}/{PROFILE_DIR}/{wallet_file}", abi.triple)
    };
    let proxy_library = |abi: &binding_layer::AndroidAbi| {
        format!("{target}/{}/{PROFILE_DIR}/{proxy_file}", abi.triple)
    };
    let wallet_crate_dir = roots.run_path(binding_layer::WALLET_CRATE_DIR);
    let proxy_crate_dir = roots.run_path(binding_layer::PROXY_CRATE_DIR);
    let inputs = binding_layer::BindgenInputs {
        language: binding_layer::KOTLIN,
        profile: BUILDER_PROFILE,
        wallet_workspace: &roots.run_path(&format!("{WALLET_WORKSPACE_DIR}/{MANIFEST}")),
        target_dir: &target,
    };
    let bindgen = |generation, library: &str, env: &[(String, String)]| {
        bindgen_step(roots, generation, &inputs, library, &kotlin_out, env)
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
    let ndk_build = |abi: &binding_layer::AndroidAbi,
                     workdir: &str,
                     target_dir: &str,
                     profile: binding_layer::Profile,
                     package: &[&str]| {
        cargo_step(
            workdir,
            &abi.env(target_dir),
            &["ndk", "--target", abi.triple, "build"],
            profile,
            package,
        )
    };
    let bindgen_abi = &binding_layer::ANDROID_ABIS[FIRST_POSITION];
    let generate_once = |position, generation, library: String| {
        (position == FIRST_POSITION)
            .then(|| bindgen(generation, &library, &bindgen_abi.env(&target)))
    };
    let wallet_steps = abis.iter().enumerate().flat_map(|(position, abi)| {
        [ndk_build(
            abi,
            &wallet_crate_dir,
            &target,
            BUILDER_PROFILE,
            &[],
        )]
        .into_iter()
        .chain(generate_once(
            position,
            binding_layer::Generation::Wallet,
            wallet_library(abi),
        ))
        .chain(strip(wallet_library(abi), &abi.env(&target)))
    });
    let proxy_steps = abis.iter().enumerate().flat_map(|(position, abi)| {
        [ndk_build(
            abi,
            &proxy_crate_dir,
            &target,
            BUILDER_PROFILE,
            &["--package", binding_layer::PROXY_PACKAGE],
        )]
        .into_iter()
        .chain(generate_once(
            position,
            binding_layer::Generation::Proxy,
            proxy_library(abi),
        ))
        .chain(strip(proxy_library(abi), &abi.env(&target)))
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
    let descriptor = abis.first().map(|abi| {
        describe_step(
            &wallet_crate_dir,
            &abi.env(&target),
            Some(abi.ndk_link()),
            abi.triple,
            BUILDER_PROFILE,
            &[],
            roots.out_host_path(binding_layer::DESCRIPTOR_FILE),
        )
    });
    wallet_steps
        .chain(proxy_steps)
        .chain(host_copies)
        .chain(descriptor)
        .collect()
}

const HOST_PROFILE: binding_layer::Profile = binding_layer::Profile::Debug;

fn kotlin_plan(roots: &Roots) -> Vec<Step> {
    let target = roots.run_path(&format!("{BUILD_ROOT}/host"));
    let kotlin_out = roots.out_run_path(binding_layer::KOTLIN_OUT_DIR);
    let inputs = binding_layer::BindgenInputs {
        language: binding_layer::KOTLIN,
        profile: HOST_PROFILE,
        wallet_workspace: &roots.run_path(&format!("{WALLET_WORKSPACE_DIR}/{MANIFEST}")),
        target_dir: &target,
    };
    let bindgen = |generation, library: &str| {
        bindgen_step(roots, generation, &inputs, library, &kotlin_out, &[])
    };
    let build = |crate_dir: &str, target_dir: &str, package: &[&str]| {
        cargo_step(
            &roots.run_path(crate_dir),
            &[(
                binding_layer::TARGET_DIR_VARIABLE.to_string(),
                target_dir.to_string(),
            )],
            &["build", "--locked"],
            HOST_PROFILE,
            package,
        )
    };
    let host_library = |lib_name, target_dir: &str| {
        binding_layer::host_library(lib_name, target_dir, HOST_PROFILE)
    };
    fresh_plan(
        roots,
        &[],
        vec![
            build(binding_layer::WALLET_CRATE_DIR, &target, &[]),
            bindgen(
                binding_layer::Generation::Wallet,
                &host_library(binding_layer::WALLET_LIB_NAME, &target),
            ),
            build(
                binding_layer::PROXY_CRATE_DIR,
                &target,
                &["--package", binding_layer::PROXY_PACKAGE],
            ),
            bindgen(
                binding_layer::Generation::Proxy,
                &host_library(binding_layer::PROXY_LIB_NAME, &target),
            ),
        ],
    )
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

fn ios_plan(roots: &Roots, phase: IosPhase) -> Vec<Step> {
    let env = vec![(
        "IPHONEOS_DEPLOYMENT_TARGET".to_string(),
        IOS_DEPLOYMENT_TARGET.to_string(),
    )];
    let wallet_target = roots.run_path(&format!(
        "{BUILD_ROOT}/ios/{}",
        binding_layer::Generation::Wallet.names().label
    ));
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
    let wallet_crate_dir = roots.run_path(binding_layer::WALLET_CRATE_DIR);
    let wallet_workspace_dir = roots.run_path(WALLET_WORKSPACE_DIR);
    let wallet_static = binding_layer::library_file(
        binding_layer::LIBRARY_PREFIX,
        binding_layer::WALLET_LIB_NAME,
        binding_layer::STATIC_SUFFIX,
    );
    let slices = roots.run_path(&format!("{BUILD_ROOT}/ios/{SLICES_DIR}"));
    let slice_dir = |triple: &str| format!("{slices}/{triple}");
    let slice = |triple: &str, file: &str| format!("{slices}/{triple}/{file}");
    let hidden = |triple: &str, file: &str| slice(triple, &format!("{HIDDEN_DIR}/{file}"));
    let universal = |file: &str| slice(UNIVERSAL_SIMULATOR_DIR, file);
    let generated = format!(
        "{GENERATED_DIR}/{}",
        binding_layer::Generation::Wallet.names().label
    );
    let headers = format!("{GENERATED_DIR}/headers");
    let hide = || {
        let object_file = binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            binding_layer::WALLET_LIB_NAME,
            OBJECT_SUFFIX,
        );
        // One pattern pair per crate. The wallet's pair covers the proxy's
        // exports by prefix today, which the layer's unit test pins as
        // incidental, so a renamed proxy stays exported.
        let exports = binding_layer::GENERATIONS
            .into_iter()
            .flat_map(|generation| {
                binding_layer::exported_symbol_patterns(generation.names().lib_name)
            })
            .flat_map(|pattern| [EXPORTED_SYMBOL_FLAG.to_string(), pattern])
            .collect::<Vec<_>>();
        iter_ios_targets()
            .flat_map(|target| {
                let archive = slice(target.triple(), &wallet_static);
                let hidden_archive = hidden(target.triple(), &wallet_static);
                let object = hidden(target.triple(), &object_file);
                [
                    Step::Run {
                        workdir: roots.run.clone(),
                        env: env.clone(),
                        command: [
                            LD,
                            RELOCATABLE_FLAG,
                            ARCH_FLAG,
                            target.arch(),
                            PLATFORM_VERSION_FLAG,
                            target.platform(),
                            IOS_DEPLOYMENT_TARGET,
                            IOS_DEPLOYMENT_TARGET,
                            FORCE_LOAD_FLAG,
                            &archive,
                        ]
                        .map(String::from)
                        .into_iter()
                        .chain(exports.iter().cloned())
                        .chain([OUTPUT_FLAG.to_string(), object.clone()])
                        .collect(),
                    },
                    Step::Run {
                        workdir: roots.run.clone(),
                        env: env.clone(),
                        command: [
                            LIBTOOL,
                            LIBTOOL_STATIC_FLAG,
                            OUTPUT_FLAG,
                            &hidden_archive,
                            &object,
                        ]
                        .map(String::from)
                        .to_vec(),
                    },
                ]
            })
            .collect::<Vec<_>>()
    };
    let inputs = binding_layer::BindgenInputs {
        language: binding_layer::SWIFT,
        profile: BUILDER_PROFILE,
        wallet_workspace: &format!("{wallet_workspace_dir}/{MANIFEST}"),
        target_dir: &wallet_target,
    };
    let bindgen = |library: &str, out: &str| {
        bindgen_step(
            roots,
            binding_layer::Generation::Wallet,
            &inputs,
            library,
            &roots.out_run_path(out),
            &env,
        )
    };
    let lipo = |file: &str| Step::Run {
        workdir: roots.run.clone(),
        env: env.clone(),
        command: ["lipo".to_string(), "-create".to_string()]
            .into_iter()
            .chain(
                IOS_SIMULATORS
                    .iter()
                    .map(|target| hidden(target.triple(), file)),
            )
            .chain(["-output".to_string(), universal(file)])
            .collect(),
    };
    let xcframework = |libraries: Vec<String>| {
        let output = roots.out_run_path(binding_layer::WALLET_XCFRAMEWORK);
        let header_args = vec!["-headers".to_string(), roots.out_run_path(&headers)];
        [
            Step::Remove(roots.out_host_path(binding_layer::WALLET_XCFRAMEWORK)),
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
    let slice_steps = |target: IosTarget| {
        let triple = target.triple();
        let built = format!(
            "{wallet_target}/{triple}/{}/{wallet_static}",
            BUILDER_PROFILE.directory()
        );
        let describe = (target == IosTarget::Device).then(|| {
            describe_step(
                &wallet_crate_dir,
                &with_target(&wallet_target),
                None,
                triple,
                BUILDER_PROFILE,
                &binding_layer::PROXY_FEATURE_ARGS,
                host_of(roots, &slice(triple, binding_layer::DESCRIPTOR_FILE)),
            )
        });
        [
            vec![Step::FreshDir(host_of(roots, &slice_dir(triple)))],
            vec![
                cargo_step(
                    &wallet_crate_dir,
                    &with_target(&wallet_target),
                    &["build", "--target", triple],
                    BUILDER_PROFILE,
                    &binding_layer::PROXY_FEATURE_ARGS,
                ),
                Step::Copy {
                    from: host_of(roots, &built),
                    to: host_of(roots, &slice(triple, &wallet_static)),
                },
            ],
            describe.into_iter().collect(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<Step>>()
    };
    let assembly_steps = || {
        let steps: Vec<Vec<Step>> = vec![
            iter_ios_targets()
                .flat_map(|target| {
                    iter::once(wallet_static.as_str())
                        .chain(
                            (target == IosTarget::Device).then_some(binding_layer::DESCRIPTOR_FILE),
                        )
                        .map(move |file| {
                            Step::Require(host_of(roots, &slice(target.triple(), file)))
                        })
                })
                .collect(),
            vec![
                Step::FreshDir(host(&generated)),
                Step::FreshDir(host(&headers)),
                Step::FreshDir(host_of(roots, &slice_dir(UNIVERSAL_SIMULATOR_DIR))),
            ],
            iter_ios_targets()
                .map(|target| Step::FreshDir(host_of(roots, &slice(target.triple(), HIDDEN_DIR))))
                .collect(),
            vec![bindgen(
                &slice(IosTarget::Device.triple(), &wallet_static),
                &generated,
            )],
            hide(),
            vec![lipo(&wallet_static)],
            [WALLET_HEADER, PROXY_HEADER]
                .map(|header| Step::Copy {
                    from: host(&format!("{generated}/{header}")),
                    to: host(&format!("{headers}/{header}")),
                })
                .into_iter()
                .chain(iter::once(Step::Concatenate {
                    sources: [WALLET_MODULEMAP, PROXY_MODULEMAP]
                        .map(|modulemap| host(&format!("{generated}/{modulemap}")))
                        .to_vec(),
                    separator: MODULEMAP_SEPARATOR,
                    to: host(&format!("{headers}/{COMBINED_MODULEMAP}")),
                }))
                .collect(),
            xcframework(vec![
                hidden(IosTarget::Device.triple(), &wallet_static),
                universal(&wallet_static),
            ])
            .into_iter()
            .collect(),
            binding_layer::SWIFT_SOURCES
                .map(|swift| Step::Copy {
                    from: host(&format!("{generated}/{swift}")),
                    to: host(&format!("{}/{swift}", binding_layer::swift_sources_dir())),
                })
                .into_iter()
                .collect(),
            vec![Step::Copy {
                from: host_of(
                    roots,
                    &slice(IosTarget::Device.triple(), binding_layer::DESCRIPTOR_FILE),
                ),
                to: host(binding_layer::DESCRIPTOR_FILE),
            }],
            vec![Step::XcodeTest {
                workdir: roots.run_path(binding_layer::SWIFT_CONTRACT_TESTS_DIR),
                scheme: binding_layer::swift_contract_tests_scheme(),
            }],
        ];
        steps.into_iter().flatten().collect::<Vec<Step>>()
    };
    match phase {
        IosPhase::Full => fresh_plan(
            roots,
            &iter_ios_targets()
                .map(|target| target.triple())
                .collect::<Vec<_>>(),
            iter_ios_targets()
                .flat_map(&slice_steps)
                .chain(assembly_steps())
                .collect(),
        ),
        IosPhase::Slice(target) => toolchain_plan(roots, &[target.triple()], slice_steps(target)),
        IosPhase::Assembly => fresh_plan(roots, &[], assembly_steps()),
    }
}

/// The device target followed by the simulator targets.
fn iter_ios_targets() -> impl Iterator<Item = IosTarget> {
    iter::once(IosTarget::Device).chain(IOS_SIMULATORS)
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
        Step::Require(file) => file.is_file().then_some(()).ok_or_else(|| {
            vec![format!(
                "{} is missing; build its slice before the assembly",
                file.display()
            )]
        }),
        Step::Describe {
            workdir,
            env,
            ndk,
            command,
            to,
        } => {
            let env = match ndk {
                Some(link) => {
                    let exported = capture_command(runner, workdir, env, &link.env_command)?;
                    let exported = binding_layer::env_in_json(&exported).map_err(|e| vec![e])?;
                    let link_env = binding_layer::ndk_link_env(exported, &link.link_target)
                        .map_err(|e| vec![e])?;
                    [env.clone(), link_env].concat()
                }
                None => env.clone(),
            };
            let messages = capture_command(runner, workdir, &env, command)?;
            let descriptor =
                binding_layer::descriptor_in_messages(&messages)?.ok_or_else(|| {
                    vec![format!(
                        "`{}` reported no {} for zingolib",
                        command.join(" "),
                        binding_layer::DESCRIPTOR_ENV
                    )]
                })?;
            workbench::create_parent(to)?;
            fs::write(to, format!("{descriptor}\n"))
                .map_err(|e| vec![format!("cannot write {}: {e}", to.display())])
        }
        Step::XcodeTest { workdir, scheme } => {
            let listing = capture_command(
                runner,
                workdir,
                &[],
                &SIMCTL_LIST_AVAILABLE_DEVICES.map(String::from),
            )?;
            let simulator = binding_layer::ios_simulator(&listing, IOS_DEPLOYMENT_TARGET)?;
            println!("testing on {} ({})", simulator.name, simulator.udid);
            let command = xcodebuild_test(scheme, &simulator.udid);
            let (log, status) = stream_command(runner, workdir, &[], &command)?;
            if !status.success() {
                return Err(failure_of(&command, status));
            }
            let executed = binding_layer::executed_tests(&log)?;
            if executed == binding_layer::SWIFT_CONTRACT_TEST_COUNT {
                Ok(())
            } else {
                Err(vec![format!(
                    "xcodebuild executed {executed} tests, and the Swift contract suite holds {}",
                    binding_layer::SWIFT_CONTRACT_TEST_COUNT
                )])
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

fn require_host_workdir(runner: &Runner, started: &Invocation) -> Result<(), Vec<String>> {
    if matches!(runner, Runner::Host) && !path::Path::new(&started.workdir).is_dir() {
        return Err(vec![format!(
            "the step's working directory {} is absent",
            started.workdir
        )]);
    }
    Ok(())
}

fn failure_of(command: &[String], status: process::ExitStatus) -> Vec<String> {
    vec![format!("`{}` failed ({status})", command.join(" "))]
}

fn output_of(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<process::Output, Vec<String>> {
    let started = invocation(runner, workdir, env, command);
    require_host_workdir(runner, &started)?;
    host_command(&started)
        .stderr(process::Stdio::inherit())
        .output()
        .map_err(|e| vec![format!("cannot run {}: {e}", started.program)])
}

fn capture_command(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<String, Vec<String>> {
    let output = output_of(runner, workdir, env, command)?;
    if !output.status.success() {
        return Err(failure_of(command, output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| {
        vec![format!(
            "`{}` wrote output that is not UTF-8: {e}",
            command.join(" ")
        )]
    })
}

/// - Prints every line of the command's standard output as it arrives.
fn stream_command(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<(String, process::ExitStatus), Vec<String>> {
    let started = invocation(runner, workdir, env, command);
    require_host_workdir(runner, &started)?;
    let mut child = host_command(&started)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::inherit())
        .spawn()
        .map_err(|e| vec![format!("cannot run {}: {e}", started.program)])?;
    let piped = workbench::required(child.stdout.take(), "the piped standard output")?;
    let mut reader = io::BufReader::new(piped);
    let mut bytes = Vec::new();
    let mut log = String::new();
    loop {
        bytes.clear();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| vec![format!("cannot read from {}: {e}", started.program)])?;
        if read == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&bytes);
        print!("{line}");
        log.push_str(&line);
    }
    let status = child
        .wait()
        .map_err(|e| vec![format!("cannot wait for {}: {e}", started.program)])?;
    Ok((log, status))
}

/// Run one command on the runner, streaming its output, and fail if it fails.
fn run_command(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<(), Vec<String>> {
    let started = invocation(runner, workdir, env, command);
    require_host_workdir(runner, &started)?;
    let status = host_command(&started)
        .status()
        .map_err(|e| vec![format!("cannot run {}: {e}", started.program)])?;
    if status.success() {
        Ok(())
    } else {
        Err(failure_of(command, status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GENERATIONS_PER_BUILD: usize = binding_layer::GENERATIONS.len();

    const INSTALL: &str = "rustup toolchain install";

    const TARGET_ADD: &str = "rustup target add";

    const CONTAINER_ID: &str = "container-id";

    fn host_roots() -> Result<Roots, Vec<String>> {
        Roots::on_host(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        )
    }

    fn every_plan() -> Result<[Vec<Step>; 3], Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        Ok([
            android_plan(&roots()?, &abis),
            ios_plan(&host_roots()?, IosPhase::Full),
            kotlin_plan(&host_roots()?),
        ])
    }

    fn position_of(commands: &[String], leading: &str) -> Option<usize> {
        commands
            .iter()
            .position(|command| command.starts_with(leading))
    }

    #[test]
    fn every_plan_clears_its_output_then_installs_the_pin_before_its_first_cargo_step(
    ) -> Result<(), Vec<String>> {
        for plan in every_plan()? {
            assert!(matches!(plan.first(), Some(Step::FreshDir(_))));
            let commands = commands(&plan);
            let install = workbench::required(
                commands.iter().position(|command| command == INSTALL),
                "toolchain install step",
            )?;
            let cargo = workbench::required(position_of(&commands, "cargo "), "cargo step")?;
            assert!(install < cargo);
            assert!(commands.iter().all(|command| !command.contains("stable")));
        }
        Ok(())
    }

    #[test]
    fn android_plan_adds_only_the_selected_targets_to_the_pinned_toolchain(
    ) -> Result<(), Vec<String>> {
        let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
            .iter()
            .filter(|abi| abi.jni_dir == "x86")
            .collect();
        let commands = commands(&android_plan(&roots()?, &x86));
        let target_add =
            workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
        assert_eq!(
            commands[target_add],
            format!("{TARGET_ADD} {}", x86[FIRST_POSITION].triple)
        );
        Ok(())
    }

    #[test]
    fn ios_plan_adds_the_device_and_simulator_targets_to_the_pinned_toolchain(
    ) -> Result<(), Vec<String>> {
        let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
        let target_add =
            workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
        let targets: Vec<&str> = iter_ios_targets().map(|target| target.triple()).collect();
        assert_eq!(
            commands[target_add],
            format!("{TARGET_ADD} {}", targets.join(" "))
        );
        Ok(())
    }

    fn ios_static_library(lib_name: &str) -> String {
        binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            lib_name,
            binding_layer::STATIC_SUFFIX,
        )
    }

    #[test]
    fn the_ios_plan_hides_every_symbol_but_the_uniffi_exports_of_both_crates(
    ) -> Result<(), Vec<String>> {
        let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
        let archive_file = ios_static_library(binding_layer::WALLET_LIB_NAME);
        let object_file = binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            binding_layer::WALLET_LIB_NAME,
            OBJECT_SUFFIX,
        );
        let exports = [
            binding_layer::WALLET_LIB_NAME,
            binding_layer::PROXY_LIB_NAME,
        ]
        .map(|lib_name| {
            format!("-exported_symbol _uniffi_{lib_name}_* -exported_symbol _ffi_{lib_name}_*")
        })
        .join(" ");
        for target in iter_ios_targets() {
            let slice_dir = format!("{}/{}", ios_slices(), target.triple());
            let archive = format!("{slice_dir}/{archive_file}");
            let hidden_archive = format!("{slice_dir}/{HIDDEN_DIR}/{archive_file}");
            let object = format!("{slice_dir}/{HIDDEN_DIR}/{object_file}");
            let link = format!(
                "ld -r -arch {} -platform_version {} {IOS_DEPLOYMENT_TARGET} {IOS_DEPLOYMENT_TARGET} -force_load {archive} {exports} -o {object}",
                target.arch(),
                target.platform()
            );
            let pack = format!("libtool -static -o {hidden_archive} {object}");
            let link_at = workbench::required(
                commands.iter().position(|command| command == &link),
                "the relocatable link",
            )?;
            let pack_at = workbench::required(
                commands.iter().position(|command| command == &pack),
                "the archive step",
            )?;
            let bindgen_at = commands
                .iter()
                .position(|command| command.contains(&format!("--library {archive} ")));
            assert!(link_at < pack_at);
            if target == IosTarget::Device {
                assert!(workbench::required(bindgen_at, "the bindgen step")? < link_at);
            } else {
                assert!(bindgen_at.is_none());
            }
            let mut readers = 0;
            for (at, command) in commands.iter().enumerate() {
                if command.contains(&archive) {
                    assert!(
                        [Some(link_at), bindgen_at].contains(&Some(at)),
                        "{command} reads the raw {archive}"
                    );
                }
                if command.contains(&hidden_archive) && at != pack_at {
                    assert!(
                        pack_at < at,
                        "{command} reads {hidden_archive} before it exists"
                    );
                    readers += 1;
                }
            }
            assert!(readers > 0, "nothing reads {hidden_archive}");
        }
        Ok(())
    }

    #[test]
    fn the_ios_plan_ships_one_rust_archive_so_the_app_links_one_std() -> Result<(), Vec<String>> {
        let commands = commands(&ios_plan(&host_roots()?, IosPhase::Full));
        let proxy_archive = ios_static_library(binding_layer::PROXY_LIB_NAME);
        assert!(commands
            .iter()
            .all(|command| !command.contains(&proxy_archive)));
        for target in iter_ios_targets() {
            let in_slice = format!("/{}/", target.triple());
            let links = commands
                .iter()
                .filter(|command| command.starts_with(&format!("{LD} ")))
                .filter(|command| command.contains(&in_slice))
                .count();
            assert_eq!(links, 1, "{} merges {links} archives", target.triple());
        }
        let xcframeworks = commands
            .iter()
            .filter(|command| command.starts_with("xcodebuild -create-xcframework"))
            .count();
        assert_eq!(xcframeworks, 1);
        Ok(())
    }

    #[test]
    fn the_ios_plan_ends_with_the_swift_contract_test() -> Result<(), Vec<String>> {
        let plan = ios_plan(&host_roots()?, IosPhase::Full);
        let Some(Step::XcodeTest { workdir, scheme }) = plan.last() else {
            return Err(vec![
                "the iOS plan does not end with the Swift contract test".to_string(),
            ]);
        };
        assert_eq!(
            workdir,
            &format!("{HOST_ROOT}/{}", binding_layer::SWIFT_CONTRACT_TESTS_DIR)
        );
        assert_eq!(scheme, &binding_layer::swift_contract_tests_scheme());
        Ok(())
    }

    const IOS_WORKFLOW: &str = ".github/workflows/binding-layer-ios.yaml";
    const XCODE_VERSION_KEY: &str = "XCODE_VERSION:";
    const XCODE_VERSION_INPUT: &str = "xcode-version:";
    const XCODE_VERSION_REFERENCE: &str = "${{ env.XCODE_VERSION }}";
    const IOS_CACHE_KEY: &str = "key: ios-";
    const IOS_RESTORE_KEY: &str = "ios-";
    const MATRIX_TARGETS_KEY: &str = "target: [";
    const CARGO_BUILD: &str = "cargo build";
    const LIBRARY_FLAG: &str = "--library";
    const ASSEMBLY_PROGRAMS: [&str; 4] = ["ld ", "libtool ", "lipo ", "xcodebuild "];

    fn ios_slices() -> String {
        format!("{HOST_ROOT}/{BUILD_ROOT}/ios/{SLICES_DIR}")
    }

    fn copies(plan: &[Step]) -> Vec<(String, String)> {
        plan.iter()
            .filter_map(|step| match step {
                Step::Copy { from, to } => Some((
                    from.to_string_lossy().to_string(),
                    to.to_string_lossy().to_string(),
                )),
                _ => None,
            })
            .collect()
    }

    fn after_the_toolchain(commands: Vec<String>) -> Vec<String> {
        let first = commands
            .iter()
            .rposition(|command| command.starts_with("rustup"))
            .map_or(0, |at| at + 1);
        commands[first..].to_vec()
    }

    #[test]
    fn the_ios_slice_plan_builds_one_triple_and_delivers_its_archive() -> Result<(), Vec<String>> {
        for target in iter_ios_targets() {
            let triple = target.triple();
            let plan = ios_plan(&host_roots()?, IosPhase::Slice(target));
            let commands = commands(&plan);
            let target_add =
                workbench::required(position_of(&commands, TARGET_ADD), "target add step")?;
            assert_eq!(commands[target_add], format!("{TARGET_ADD} {triple}"));
            let builds: Vec<&String> = commands
                .iter()
                .filter(|command| command.starts_with(CARGO_BUILD))
                .collect();
            let [build] = builds[..] else {
                return Err(vec![format!(
                    "one cargo build per slice, found {}",
                    builds.len()
                )]);
            };
            assert!(build.contains(&format!("--target {triple}")));
            assert!(
                build.ends_with(&binding_layer::PROXY_FEATURE_ARGS.join(" ")),
                "{build} leaves the proxy crate out of the archive"
            );
            assert!(commands.iter().all(|command| {
                !command.contains(LIBRARY_FLAG)
                    && ASSEMBLY_PROGRAMS
                        .iter()
                        .all(|program| !command.starts_with(program))
            }));
            let delivered: Vec<String> = copies(&plan).into_iter().map(|(_, to)| to).collect();
            assert_eq!(
                delivered,
                vec![format!(
                    "{}/{triple}/{}",
                    ios_slices(),
                    ios_static_library(binding_layer::WALLET_LIB_NAME)
                )]
            );
            let describes = plan
                .iter()
                .filter(|step| matches!(step, Step::Describe { .. }))
                .count();
            assert_eq!(describes, usize::from(target == IosTarget::Device));
            assert!(!matches!(plan.last(), Some(Step::XcodeTest { .. })));
        }
        Ok(())
    }

    #[test]
    fn the_ios_slice_plan_leaves_the_package_output_alone() -> Result<(), Vec<String>> {
        let roots = host_roots()?;
        let output = format!("{}/", roots.out_host.to_string_lossy());
        for target in iter_ios_targets() {
            for step in ios_plan(&roots, IosPhase::Slice(target)) {
                if let Step::FreshDir(dir) | Step::Remove(dir) = step {
                    let dir = dir.to_string_lossy();
                    assert!(
                        !dir.starts_with(&output) && dir != roots.out_host.to_string_lossy(),
                        "a slice clears {dir}"
                    );
                }
            }
        }
        Ok(())
    }

    #[test]
    fn the_ios_assembly_plan_requires_every_slice_before_it_reads_one() -> Result<(), Vec<String>> {
        let plan = ios_plan(&host_roots()?, IosPhase::Assembly);
        let required: Vec<String> = plan
            .iter()
            .filter_map(|step| match step {
                Step::Require(file) => Some(file.to_string_lossy().to_string()),
                _ => None,
            })
            .collect();
        let mut expected = Vec::new();
        for target in iter_ios_targets() {
            expected.push(format!(
                "{}/{}/{}",
                ios_slices(),
                target.triple(),
                ios_static_library(binding_layer::WALLET_LIB_NAME)
            ));
            if target == IosTarget::Device {
                expected.push(format!(
                    "{}/{}/{}",
                    ios_slices(),
                    target.triple(),
                    binding_layer::DESCRIPTOR_FILE
                ));
            }
        }
        assert_eq!(required, expected);
        let last_require = workbench::required(
            plan.iter()
                .rposition(|step| matches!(step, Step::Require(_))),
            "a require step",
        )?;
        let first_read = workbench::required(
            plan.iter().position(|step| {
                matches!(step, Step::Run { command, .. } if !command[0].starts_with("rustup"))
                    || matches!(step, Step::Copy { .. })
            }),
            "a step that reads a slice",
        )?;
        assert!(
            last_require < first_read,
            "a slice is read before every slice is required"
        );
        Ok(())
    }

    #[test]
    fn the_ios_assembly_plan_compiles_no_target_and_reads_only_the_slices(
    ) -> Result<(), Vec<String>> {
        let plan = ios_plan(&host_roots()?, IosPhase::Assembly);
        let commands = commands(&plan);
        assert!(position_of(&commands, TARGET_ADD).is_none());
        assert!(commands
            .iter()
            .all(|command| !command.starts_with(CARGO_BUILD)));
        for generation in binding_layer::GENERATIONS {
            for target in iter_ios_targets() {
                let built = format!(
                    "{HOST_ROOT}/{BUILD_ROOT}/ios/{}/{}",
                    generation.names().label,
                    target.triple()
                );
                assert!(
                    commands.iter().all(|command| !command.contains(&built)),
                    "a command reads {built}"
                );
            }
        }
        assert!(plan
            .iter()
            .all(|step| !matches!(step, Step::Describe { .. })));
        let descriptor = (
            format!(
                "{}/{}/{}",
                ios_slices(),
                IosTarget::Device.triple(),
                binding_layer::DESCRIPTOR_FILE
            ),
            format!("{OUTSIDE_OUT}/{}", binding_layer::DESCRIPTOR_FILE),
        );
        assert!(copies(&plan).contains(&descriptor));
        assert!(matches!(plan.last(), Some(Step::XcodeTest { .. })));
        Ok(())
    }

    fn rendered(plan: &[Step]) -> Vec<String> {
        let paths = |paths: &[path::PathBuf]| {
            paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        };
        plan.iter()
            .map(|step| match step {
                Step::Run { command, .. } => command.join(" "),
                Step::Copy { from, to } => format!("copy {}", paths(&[from.clone(), to.clone()])),
                Step::Concatenate { sources, to, .. } => {
                    format!("concatenate {} {}", paths(sources), to.to_string_lossy())
                }
                Step::FreshDir(dir) => format!("fresh {}", dir.to_string_lossy()),
                Step::Remove(dir) => format!("remove {}", dir.to_string_lossy()),
                Step::Require(file) => format!("require {}", file.to_string_lossy()),
                Step::Describe { command, to, .. } => {
                    format!("describe {} {}", command.join(" "), to.to_string_lossy())
                }
                Step::XcodeTest { workdir, scheme } => format!("xcodetest {workdir} {scheme}"),
            })
            .collect()
    }

    #[test]
    fn the_full_ios_plan_is_every_slice_plan_followed_by_the_assembly() -> Result<(), Vec<String>> {
        let roots = host_roots()?;
        let mut expected = Vec::new();
        for target in iter_ios_targets() {
            expected.extend(after_the_toolchain(rendered(&ios_plan(
                &roots,
                IosPhase::Slice(target),
            ))));
        }
        expected.extend(after_the_toolchain(rendered(&ios_plan(
            &roots,
            IosPhase::Assembly,
        ))));
        assert_eq!(
            after_the_toolchain(rendered(&ios_plan(&roots, IosPhase::Full))),
            expected
        );
        Ok(())
    }

    #[test]
    fn the_ios_flags_select_one_slice_or_the_assembly() -> Result<(), Vec<String>> {
        let simulator = IosTarget::Simulator(IosArch::Arm64);
        let Arguments { platform, .. } =
            parse(&["ios", TARGET_FLAG, simulator.triple()].map(String::from))?;
        assert!(matches!(platform, Platform::Ios(IosPhase::Slice(target)) if target == simulator));
        let Arguments { platform, .. } = parse(&["ios", ASSEMBLE_FLAG].map(String::from))?;
        assert!(matches!(platform, Platform::Ios(IosPhase::Assembly)));
        let both = workbench::refused(parse(
            &["ios", TARGET_FLAG, simulator.triple(), ASSEMBLE_FLAG].map(String::from),
        ))?
        .concat();
        assert!(both.contains(TARGET_FLAG) && both.contains(ASSEMBLE_FLAG));
        let android_triple = binding_layer::ANDROID_ABIS[FIRST_POSITION].triple;
        let unknown = workbench::refused(parse(
            &["ios", TARGET_FLAG, android_triple].map(String::from),
        ))?
        .concat();
        assert!(unknown.contains(android_triple));
        for target in iter_ios_targets() {
            assert!(unknown.contains(target.triple()), "{unknown}");
        }
        assert!(unknown.contains(USAGE), "{unknown}");
        for other in ["android", "kotlin"] {
            let diagnostic = workbench::refused(parse(
                &[other, ASSEMBLE_FLAG, OUT_FLAG, "out"].map(String::from),
            ))?
            .concat();
            assert!(diagnostic.contains(ASSEMBLE_FLAG));
        }
        Ok(())
    }

    #[test]
    fn the_ios_workflow_pins_one_xcode_and_keys_its_caches_by_it() -> Result<(), Vec<String>> {
        let workflow = workbench::read(&workbench::repo_root()?.join(IOS_WORKFLOW))?;
        let lines: Vec<&str> = workflow.lines().map(str::trim).collect();
        let pins: Vec<&str> = lines
            .iter()
            .filter(|line| line.starts_with(XCODE_VERSION_KEY))
            .map(|line| {
                line.trim_start_matches(XCODE_VERSION_KEY)
                    .trim()
                    .trim_matches('\'')
            })
            .collect();
        let [version] = pins[..] else {
            return Err(vec![format!(
                "{IOS_WORKFLOW} pins {} Xcode versions, not one",
                pins.len()
            )]);
        };
        assert!(
            version
                .split('.')
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())),
            "`{version}` is no explicit Xcode version"
        );
        assert_eq!(version, binding_layer::IOS_XCODE_VERSION);
        let selections: Vec<&&str> = lines
            .iter()
            .filter(|line| line.starts_with(XCODE_VERSION_INPUT))
            .collect();
        assert!(!selections.is_empty());
        for selection in selections {
            assert_eq!(
                *selection,
                format!("{XCODE_VERSION_INPUT} {XCODE_VERSION_REFERENCE}")
            );
        }
        let keys: Vec<&&str> = lines
            .iter()
            .filter(|line| line.starts_with(IOS_CACHE_KEY) || line.starts_with(IOS_RESTORE_KEY))
            .collect();
        assert_eq!(keys.len(), 6);
        for key in keys {
            assert!(
                key.contains(XCODE_VERSION_REFERENCE),
                "`{key}` is not keyed by the Xcode version"
            );
        }
        Ok(())
    }

    #[test]
    fn the_ios_workflow_matrix_names_every_ios_target() -> Result<(), Vec<String>> {
        let workflow = workbench::read(&workbench::repo_root()?.join(IOS_WORKFLOW))?;
        let line = workflow
            .lines()
            .map(str::trim)
            .find(|line| line.starts_with(MATRIX_TARGETS_KEY))
            .ok_or_else(|| vec![format!("{IOS_WORKFLOW} lists no matrix targets")])?;
        let listed: Vec<&str> = line
            .trim_start_matches(MATRIX_TARGETS_KEY)
            .trim_end_matches(']')
            .split(',')
            .map(str::trim)
            .collect();
        let known: Vec<&str> = iter_ios_targets().map(|target| target.triple()).collect();
        assert_eq!(listed, known);
        Ok(())
    }

    const OUTPUT_FLAGS: [&str; 2] = ["-o", "-output"];

    fn written_paths(step: &Step) -> Vec<String> {
        match step {
            Step::Run { command, .. } => command
                .windows(2)
                .filter(|pair| OUTPUT_FLAGS.contains(&pair[0].as_str()))
                .map(|pair| pair[1].clone())
                .collect(),
            Step::Copy { to, .. } | Step::Concatenate { to, .. } | Step::Describe { to, .. } => {
                vec![to.to_string_lossy().to_string()]
            }
            Step::FreshDir(_) | Step::Remove(_) | Step::Require(_) | Step::XcodeTest { .. } => {
                vec![]
            }
        }
    }

    #[test]
    fn no_plan_recreates_a_directory_an_earlier_step_wrote_into() -> Result<(), Vec<String>> {
        let roots = host_roots()?;
        let plans = [
            ios_plan(&roots, IosPhase::Full),
            ios_plan(&roots, IosPhase::Assembly),
        ];
        for plan in plans.iter().chain(every_plan()?.iter()) {
            let mut written: Vec<String> = Vec::new();
            for step in plan {
                if let Step::FreshDir(dir) = step {
                    let dir = format!("{}/", dir.to_string_lossy());
                    let lost: Vec<&String> = written
                        .iter()
                        .filter(|path| path.starts_with(&dir))
                        .collect();
                    assert!(lost.is_empty(), "recreating {dir} discards {lost:?}");
                }
                written.extend(written_paths(step));
            }
        }
        Ok(())
    }

    #[test]
    fn the_ios_plan_takes_no_output_directory() -> Result<(), Vec<String>> {
        let Arguments { platform, out, .. } = parse(&["ios".to_string()])?;
        assert!(matches!(platform, Platform::Ios(IosPhase::Full)));
        assert_eq!(out, None);
        let diagnostic =
            workbench::refused(parse(&["ios", OUT_FLAG, "out"].map(String::from)))?.concat();
        assert!(diagnostic.contains(OUT_FLAG));
        Ok(())
    }

    #[test]
    fn the_other_plans_require_an_output_directory() -> Result<(), Vec<String>> {
        for platform in ["android", "kotlin"] {
            let Arguments { out, .. } = parse(&[platform, OUT_FLAG, "out"].map(String::from))?;
            assert!(out.is_some_and(|out| out.ends_with("out")));
            let Arguments { out, .. } = parse(&[platform.to_string()])?;
            let diagnostic = workbench::refused(required_out(out))?.concat();
            assert!(diagnostic.contains(OUT_FLAG));
        }
        Ok(())
    }

    #[test]
    fn the_xcodebuild_test_names_the_scheme_and_the_simulator_by_id() {
        assert_eq!(
            xcodebuild_test("Scheme-Package", "5555").join(" "),
            "xcodebuild test -scheme Scheme-Package -destination platform=iOS Simulator,id=5555,arch=arm64"
        );
    }

    #[test]
    fn kotlin_plan_adds_no_target() -> Result<(), Vec<String>> {
        assert!(position_of(&commands(&kotlin_plan(&host_roots()?)), TARGET_ADD).is_none());
        Ok(())
    }

    #[test]
    fn no_plan_step_overrides_the_toolchain_pin() -> Result<(), Vec<String>> {
        for plan in every_plan()? {
            assert!(plan.iter().all(|step| match step {
                Step::Run { env, .. } | Step::Describe { env, .. } => env
                    .iter()
                    .all(|(key, _)| key != binding_layer::TOOLCHAIN_VARIABLE),
                _ => true,
            }));
        }
        Ok(())
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
    fn the_container_command_clears_the_toolchain_override_the_image_carries(
    ) -> Result<(), Vec<String>> {
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
        let id = workbench::required(
            started.args.iter().position(|arg| arg == CONTAINER_ID),
            "container id argument",
        )?;
        assert!(started.args[..id]
            .windows(2)
            .any(|pair| pair[0] == "--env" && pair[1] == cleared));
        Ok(())
    }

    #[test]
    fn every_android_run_step_carries_the_base_environment() -> Result<(), Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots()?, &abis);
        let base = android_base_env();
        assert!(plan.iter().all(|step| match step {
            Step::Run { env, .. } | Step::Describe { env, .. } => {
                base.iter().all(|entry| env.contains(entry))
            }
            _ => true,
        }));
        Ok(())
    }

    #[test]
    fn the_android_and_ios_plans_describe_the_wallet_build_with_cargo_messages(
    ) -> Result<(), Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        for plan in [
            android_plan(&roots()?, &abis),
            ios_plan(&host_roots()?, IosPhase::Full),
        ] {
            let describes: Vec<_> = plan
                .iter()
                .filter_map(|step| match step {
                    Step::Describe {
                        workdir,
                        ndk,
                        command,
                        to,
                        ..
                    } => Some((workdir, ndk, command, to)),
                    _ => None,
                })
                .collect();
            let [(workdir, ndk, command, to)] = describes[..] else {
                return Err(vec![format!(
                    "one describe step per plan, found {}",
                    describes.len()
                )]);
            };
            assert!(workdir.ends_with(binding_layer::WALLET_CRATE_DIR));
            assert_eq!(
                &command[..3],
                ["cargo", "build", "--target"].map(String::from)
            );
            assert!(command.iter().all(|word| word != "ndk"));
            assert_eq!(
                command.last().map(String::as_str),
                Some(binding_layer::MESSAGE_FORMAT_FLAG)
            );
            assert!(to.ends_with(binding_layer::DESCRIPTOR_FILE));
            match ndk {
                Some(link) => {
                    assert_eq!(command[3], abis[FIRST_POSITION].triple);
                    assert_eq!(
                        link.env_command,
                        binding_layer::ndk_env_command(&command[3])
                    );
                }
                None => {
                    assert_eq!(command[3], IosTarget::Device.triple());
                    let described = command[..command.len() - 1].join(" ");
                    assert!(
                        commands(&plan).contains(&described),
                        "the describe step builds `{described}`, which no slice build does"
                    );
                }
            }
        }
        Ok(())
    }

    /// The zingolib root as the host sees it in tests.
    const HOST_ROOT: &str = "/host/zingolib";

    /// An output directory outside the zingolib root.
    const OUTSIDE_OUT: &str = "/host/consumer/android/build/binding-layer";

    /// An output directory inside the zingolib root.
    const INSIDE_OUT: &str = "/host/zingolib/bindings/android/build/binding-layer";

    /// The output directory that the Android workflow names, relative to the root.
    const RELATIVE_OUT: &str = "bindings/android/build/binding-layer";

    fn roots() -> Result<Roots, Vec<String>> {
        Roots::in_container(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(INSIDE_OUT),
        )
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
    fn android_plan_generates_each_binding_set_once_from_the_first_abi() -> Result<(), Vec<String>>
    {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots()?, &abis);
        let generations = commands(&plan)
            .into_iter()
            .filter(|command| command.contains("--library"))
            .count();
        assert_eq!(generations, GENERATIONS_PER_BUILD);
        Ok(())
    }

    #[test]
    fn every_bindgen_takes_the_aarch64_environment_whatever_the_selection(
    ) -> Result<(), Vec<String>> {
        let x86: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS
            .iter()
            .filter(|abi| abi.jni_dir == "x86")
            .collect();
        let plan = android_plan(&roots()?, &x86);
        let expected_cc = binding_layer::ANDROID_ABIS[FIRST_POSITION].cc();
        let bindgen_envs: Vec<_> = plan
            .iter()
            .filter_map(|step| match step {
                Step::Run { env, command, .. } if command.iter().any(|arg| arg == "--library") => {
                    Some(env.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(bindgen_envs.len(), GENERATIONS_PER_BUILD);
        assert!(bindgen_envs.iter().all(|env| env
            .iter()
            .any(|(key, value)| key == "CC" && *value == expected_cc)));
        Ok(())
    }

    #[test]
    fn kotlin_plan_generates_each_set_after_its_own_host_build() -> Result<(), Vec<String>> {
        let plan = kotlin_plan(&host_roots()?);
        let commands = commands(&plan);
        assert!(
            matches!(plan.first(), Some(Step::FreshDir(out)) if out == path::Path::new(OUTSIDE_OUT))
        );
        assert!(commands.iter().all(|command| !command.contains(" ndk ")));
        let position = |predicate: fn(&String) -> bool, what: &str| {
            workbench::required(commands.iter().position(predicate), what)
        };
        let wallet_build = position(
            |command| {
                command.starts_with("cargo build")
                    && !command.contains(binding_layer::PROXY_PACKAGE)
            },
            "wallet build step",
        )?;
        let wallet_generation = position(
            |command| {
                command.contains(&binding_layer::host_library_file(
                    binding_layer::WALLET_LIB_NAME,
                ))
            },
            "wallet generation step",
        )?;
        let proxy_build = position(
            |command| {
                command.starts_with("cargo build") && command.contains(binding_layer::PROXY_PACKAGE)
            },
            "proxy build step",
        )?;
        let proxy_generation = position(
            |command| {
                command.contains(&binding_layer::host_library_file(
                    binding_layer::PROXY_LIB_NAME,
                ))
            },
            "proxy generation step",
        )?;
        assert!(wallet_build < wallet_generation);
        assert!(wallet_generation < proxy_build);
        assert!(proxy_build < proxy_generation);
        assert_eq!(
            commands
                .iter()
                .filter(|command| command.contains("--language kotlin"))
                .count(),
            binding_layer::GENERATIONS.len()
        );
        Ok(())
    }

    #[test]
    fn kotlin_bindgens_read_the_libraries_the_host_builds_write() -> Result<(), Vec<String>> {
        let plan = kotlin_plan(&host_roots()?);
        let build_targets: Vec<_> = plan
            .iter()
            .filter_map(|step| match step {
                Step::Run { env, command, .. } if command.join(" ").starts_with("cargo build") => {
                    env.iter()
                        .find(|(key, _)| key == binding_layer::TARGET_DIR_VARIABLE)
                        .map(|(_, target)| target.clone())
                }
                _ => None,
            })
            .collect();
        let libraries: Vec<_> = plan
            .iter()
            .filter_map(|step| match step {
                Step::Run { command, .. } => command
                    .iter()
                    .position(|arg| arg == "--library")
                    .map(|flag| command[flag + 1].clone()),
                _ => None,
            })
            .collect();
        let lib_names = [
            binding_layer::WALLET_LIB_NAME,
            binding_layer::PROXY_LIB_NAME,
        ];
        assert_eq!(build_targets.len(), lib_names.len());
        assert_eq!(libraries.len(), lib_names.len());
        for ((build_target, library), lib_name) in
            build_targets.iter().zip(&libraries).zip(lib_names)
        {
            assert_eq!(
                *library,
                format!(
                    "{build_target}/{}/{}",
                    HOST_PROFILE.directory(),
                    binding_layer::host_library_file(lib_name)
                )
            );
        }
        assert!(commands(&plan)
            .iter()
            .all(|command| !command.contains("--release")));
        Ok(())
    }

    /// Tests that every Kotlin generation runs from the standalone bindgen package, whose only dependency is uniffi.
    #[test]
    fn kotlin_plan_runs_every_bindgen_from_the_standalone_package() -> Result<(), Vec<String>> {
        let generations = commands(&kotlin_plan(&host_roots()?))
            .into_iter()
            .filter(|command| command.contains("--language kotlin"))
            .collect::<Vec<_>>();
        assert_eq!(generations.len(), binding_layer::GENERATIONS.len());
        assert!(generations
            .iter()
            .all(|command| command.contains("--package zingo-uniffi-bindgen")));
        assert!(generations
            .iter()
            .all(|command| !command
                .contains(&format!("{}/{MANIFEST}", binding_layer::WALLET_CRATE_DIR))));
        Ok(())
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
    fn every_android_library_builds_under_the_one_mobile_profile() -> Result<(), Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let builds: Vec<String> = android_plan(&roots()?, &abis)
            .iter()
            .filter_map(|step| match step {
                Step::Run { command, .. } if command.iter().any(|arg| arg == "build") => {
                    Some(command.join(" "))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            builds.len(),
            abis.len() * binding_layer::BINDING_CRATE_DIRS.len(),
            "{builds:?}"
        );
        let profile = BUILDER_PROFILE.cargo_args().join(" ");
        assert!(
            builds.iter().all(|command| command.contains(&profile)),
            "a library builds under another profile than {profile}: {builds:?}"
        );
        Ok(())
    }

    #[test]
    fn every_android_library_builds_into_one_target_directory() -> Result<(), Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let mut targets: Vec<String> = android_plan(&roots()?, &abis)
            .iter()
            .filter_map(|step| match step {
                Step::Run { env, command, .. } if command.iter().any(|arg| arg == "build") => env
                    .iter()
                    .find(|(key, _)| key == binding_layer::TARGET_DIR_VARIABLE)
                    .map(|(_, target)| target.clone()),
                _ => None,
            })
            .collect();
        targets.dedup();
        assert_eq!(
            targets.len(),
            1,
            "the wallet and the proxy build into separate target directories, and every dependency they share compiles twice per ABI: {targets:?}"
        );
        Ok(())
    }

    #[test]
    fn android_plan_copies_both_libraries_for_every_abi() -> Result<(), Vec<String>> {
        let abis: Vec<&binding_layer::AndroidAbi> = binding_layer::ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots()?, &abis);
        let copies = plan
            .iter()
            .filter(|step| matches!(step, Step::Copy { .. }))
            .count();
        assert_eq!(
            copies,
            binding_layer::ANDROID_ABIS.len() * binding_layer::GENERATIONS.len()
        );
        Ok(())
    }

    #[test]
    fn the_container_refuses_an_output_directory_outside_the_mounted_root(
    ) -> Result<(), Vec<String>> {
        let diagnostic = workbench::refused(Roots::in_container(
            path::PathBuf::from(HOST_ROOT),
            path::PathBuf::from(OUTSIDE_OUT),
        ))?
        .concat();
        assert!(diagnostic.contains(OUTSIDE_OUT) && diagnostic.contains("mounts"));
        Ok(())
    }

    #[test]
    fn the_container_sees_the_output_directory_under_its_own_root() -> Result<(), Vec<String>> {
        assert_eq!(
            roots()?.out_run_path(binding_layer::KOTLIN_OUT_DIR),
            format!(
                "{CONTAINER_ROOT}/bindings/android/build/binding-layer/{}",
                binding_layer::KOTLIN_OUT_DIR
            )
        );
        Ok(())
    }

    #[test]
    fn the_container_sees_the_output_directory_with_slashes_whatever_the_host_separator(
    ) -> Result<(), Vec<String>> {
        let root = path::PathBuf::from(HOST_ROOT);
        let out = root
            .join("bindings")
            .join("android")
            .join("build")
            .join("binding-layer");
        assert_eq!(
            Roots::in_container(root, out)?.out_run,
            format!("{CONTAINER_ROOT}/{RELATIVE_OUT}")
        );
        Ok(())
    }

    #[test]
    fn the_host_takes_an_output_directory_outside_the_root() -> Result<(), Vec<String>> {
        let plan = ios_plan(&host_roots()?, IosPhase::Full);
        assert!(matches!(
            plan.first(),
            Some(Step::FreshDir(directory)) if directory == path::Path::new(OUTSIDE_OUT)
        ));
        assert!(commands(&plan)
            .iter()
            .any(|command| command.contains(&format!("{OUTSIDE_OUT}/{GENERATED_DIR}/wallet"))));
        Ok(())
    }

    /// A container engine that no host has installed.
    const ABSENT_ENGINE: &str = "absent-container-engine";

    #[test]
    fn a_container_engine_that_does_not_start_is_reported_by_name_alone() -> Result<(), Vec<String>>
    {
        let runner = Runner::Container {
            engine: ABSENT_ENGINE,
            id: "container".to_string(),
        };
        let step_workdir = format!("{CONTAINER_ROOT}/{}", binding_layer::WALLET_CRATE_DIR);
        let diagnostic = workbench::refused(run_command(
            &runner,
            &step_workdir,
            &[],
            &["cargo".to_string()],
        ))?
        .concat();
        assert!(diagnostic.contains(ABSENT_ENGINE) && !diagnostic.contains(&step_workdir));
        Ok(())
    }

    #[test]
    fn an_absent_host_working_directory_is_reported_before_the_program() -> Result<(), Vec<String>>
    {
        let step_workdir = format!("{HOST_ROOT}/{}", binding_layer::WALLET_CRATE_DIR);
        let diagnostic = workbench::refused(run_command(
            &Runner::Host,
            &step_workdir,
            &[],
            &["cargo".to_string()],
        ))?
        .concat();
        assert!(diagnostic.contains(&step_workdir) && !diagnostic.contains("cargo"));
        let captured = workbench::refused(capture_command(
            &Runner::Host,
            &step_workdir,
            &[],
            &["cargo".to_string()],
        ))?
        .concat();
        assert_eq!(captured, diagnostic);
        Ok(())
    }

    #[test]
    fn an_output_directory_holding_the_root_is_refused_on_both_runners() -> Result<(), Vec<String>>
    {
        let root = workbench::repo_root()?;
        let parent =
            workbench::required(root.parent(), "parent of the repository root")?.to_path_buf();
        assert!(Roots::on_host(root.clone(), root.clone()).is_err());
        assert!(Roots::on_host(root.clone(), parent.clone()).is_err());
        assert!(Roots::in_container(root.clone(), root.clone()).is_err());
        assert!(Roots::in_container(root.clone(), parent).is_err());
        assert!(Roots::on_host(root.clone(), root.join(RELATIVE_OUT)).is_ok());
        Ok(())
    }

    #[test]
    fn host_of_maps_container_paths_back_to_the_host() -> Result<(), Vec<String>> {
        assert_eq!(
            host_of(&roots()?, "/opt/zingolib/target/x.so"),
            path::PathBuf::from("/host/zingolib/target/x.so")
        );
        Ok(())
    }
}
