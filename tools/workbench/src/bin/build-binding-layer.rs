#![forbid(unsafe_code)]

use std::env;
use std::fs;
use std::path;
use std::process;

use workbench::binding_layer;

/// The program name that prefixes every diagnostic.
const PROGRAM: &str = "build-binding-layer";

/// The invocation shape, reported when the arguments do not parse.
const USAGE: &str =
    "usage: build-binding-layer <android|ios> --out <directory> [--abi <android abi>]";

/// The number of leading command-line arguments that name the program itself.
const PROGRAM_NAME_ARGUMENTS: usize = 1;

/// The flag that names the output directory.
const OUT_FLAG: &str = "--out";

/// The flag that selects one Android ABI instead of all of them.
const ABI_FLAG: &str = "--abi";

/// The variable that carries zingo-mobile's `git describe` into the wallet's build script.
const DESCRIBE_VARIABLE: &str = "ZINGO_MOBILE_GIT_DESCRIBE";

/// The variable that makes rustup ignore zingolib's toolchain pin, as zingo-mobile's builders do.
const TOOLCHAIN_VARIABLE: &str = "RUSTUP_TOOLCHAIN";

/// The toolchain that zingo-mobile's builders select.
const BUILDER_TOOLCHAIN: &str = "stable";

/// The variable that tells cargo where to write build output.
const TARGET_DIR_VARIABLE: &str = "CARGO_TARGET_DIR";

/// The container engines to try, in order.
const ENGINES: [&str; 2] = ["podman", "docker"];

/// The tag of the Android tool image that the builder builds and runs.
const ANDROID_IMAGE: &str = "localhost/zingolib/binding-layer-android";

/// The Dockerfile of the Android tool image, relative to the zingolib root.
const ANDROID_DOCKERFILE: &str = "bindings/android/docker/Dockerfile";

/// The build context of the Android tool image, relative to the zingolib root.
const ANDROID_CONTEXT: &str = "bindings/android/docker";

/// The directory at which the container sees the zingolib root.
const CONTAINER_ROOT: &str = "/opt/zingolib";

/// The Android API level that zingo-mobile's builder compiles against.
const ANDROID_API_LEVEL: &str = "26";

/// The iOS deployment target that zingo-mobile's builder sets.
const IOS_DEPLOYMENT_TARGET: &str = "16.0";

/// The operating system name that Rust reports on macOS.
const MACOS: &str = "macos";

/// The directory under the zingolib root that holds the builder's cargo output.
const BUILD_ROOT: &str = "target/binding-layer";

/// The wallet crate's directory, relative to the zingolib root.
const WALLET_CRATE_DIR: &str = "zingo-ffi/lib";

/// The wallet-side workspace directory, relative to the zingolib root.
const WALLET_WORKSPACE_DIR: &str = "zingo-ffi";

/// The proxy crate's directory, relative to the zingolib root.
const PROXY_CRATE_DIR: &str = "zingo-netutils/nym-proxy-ffi";

/// The manifest file name that every crate directory holds.
const MANIFEST: &str = "Cargo.toml";

/// The wallet crate's UDL file, relative to the wallet crate's directory.
const UDL: &str = "src/zingo.udl";

/// The profile that zingo-mobile's builders build every library with.
const BUILDER_PROFILE: binding_layer::Profile = binding_layer::Profile::Release;

/// The directory under a target directory that the builder profile writes to.
const PROFILE_DIR: &str = BUILDER_PROFILE.directory();

/// The Android output subdirectory that holds the Kotlin sources the AAR compiles.
const KOTLIN_OUT_DIR: &str = "kotlin";

/// The Android output subdirectory that holds the per-ABI libraries the AAR packages.
const JNI_LIBS_DIR: &str = "jniLibs";

/// The iOS output subdirectory that holds intermediate generated files.
const GENERATED_DIR: &str = "generated";

/// The name under which zingo-mobile ships the wallet's Android library.
const ANDROID_WALLET_LIBRARY: &str = "libuniffi_zingo.so";

/// The prefix of a Unix library file name.
const LIBRARY_PREFIX: &str = "lib";

/// The suffix of an Android shared library.
const SHARED_SUFFIX: &str = ".so";

/// The suffix of an iOS static library.
const STATIC_SUFFIX: &str = ".a";

/// The iOS target that builds for devices.
const IOS_DEVICE_TARGET: &str = "aarch64-apple-ios";

/// The iOS targets that `lipo` merges into one simulator library.
const IOS_SIMULATOR_TARGETS: [&str; 2] = ["aarch64-apple-ios-sim", "x86_64-apple-ios"];

/// The directory, under a target directory, that holds merged simulator libraries.
const UNIVERSAL_SIMULATOR_DIR: &str = "universal-sim";

/// The wallet XCFramework that zingo-mobile ships.
const WALLET_XCFRAMEWORK: &str = "Zingolib.xcframework";

/// The proxy XCFramework that zingo-mobile ships.
const PROXY_XCFRAMEWORK: &str = "ZingoNymProxyFFI.xcframework";

/// The Swift source directory that the SwiftPM package compiles, relative to the output directory.
const SWIFT_SOURCES_DIR: &str = "Sources/ZingoBindings";

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

/// The wallet's generated Swift source.
const WALLET_SWIFT: &str = "zingo.swift";

/// The proxy's generated Swift source.
const PROXY_SWIFT: &str = "zingo_nym_proxy_ffi.swift";

/// One Android ABI, with the environment that zingo-mobile's builder sets for it.
struct AndroidAbi {
    /// The Rust target triple.
    triple: &'static str,
    /// The directory name under `jniLibs`.
    jni_dir: &'static str,
    /// The prefix of the NDK clang wrapper, before the API level.
    clang_prefix: &'static str,
    /// The value of `CARGO_FEATURE_STD` that zingo-mobile's builder sets.
    std_feature: &'static str,
}

impl AndroidAbi {
    /// The NDK clang wrapper for this ABI at zingo-mobile's API level.
    fn cc(&self) -> String {
        format!("{}{ANDROID_API_LEVEL}-clang", self.clang_prefix)
    }

    /// The environment that zingo-mobile's builder sets while it builds this ABI.
    fn env(&self, target_dir: &str) -> Vec<(String, String)> {
        [
            ("CARGO_FEATURE_STD", self.std_feature.to_string()),
            ("CC", self.cc()),
            (TARGET_DIR_VARIABLE, target_dir.to_string()),
        ]
        .map(|(key, value)| (key.to_string(), value))
        .to_vec()
    }
}

/// Every Android ABI, in the order that zingo-mobile's builder builds them.
const ANDROID_ABIS: [AndroidAbi; 4] = [
    AndroidAbi {
        triple: "aarch64-linux-android",
        jni_dir: "arm64-v8a",
        clang_prefix: "aarch64-linux-android",
        std_feature: "true",
    },
    AndroidAbi {
        triple: "armv7-linux-androideabi",
        jni_dir: "armeabi-v7a",
        clang_prefix: "armv7a-linux-androideabi",
        std_feature: "false",
    },
    AndroidAbi {
        triple: "i686-linux-android",
        jni_dir: "x86",
        clang_prefix: "i686-linux-android",
        std_feature: "false",
    },
    AndroidAbi {
        triple: "x86_64-linux-android",
        jni_dir: "x86_64",
        clang_prefix: "x86_64-linux-android",
        std_feature: "false",
    },
];

/// The platform whose packaging to build.
#[derive(Clone, Copy)]
enum Platform {
    /// The Android libraries and Kotlin sources for the AAR.
    Android,
    /// The two XCFrameworks and Swift sources for the SwiftPM package.
    Ios,
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

/// The two roots that a plan joins paths onto.
struct Roots {
    /// The zingolib root as the host sees it.
    host: path::PathBuf,
    /// The zingolib root as `Run` steps see it.
    run: String,
}

impl Roots {
    /// A path under the root as `Run` steps see it.
    fn run_path(&self, relative: &str) -> String {
        format!("{}/{relative}", self.run)
    }

    /// A path under the root as the host sees it.
    fn host_path(&self, relative: &str) -> path::PathBuf {
        self.host.join(relative)
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
    let describe = env::var(DESCRIBE_VARIABLE)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            vec![format!(
                "{DESCRIBE_VARIABLE} must name zingo-mobile's git describe"
            )]
        })?;
    let root = workbench::repo_root()?;
    let relative_out = out
        .strip_prefix(&root)
        .map_err(|_| {
            vec![format!(
                "{} is not inside {}",
                out.display(),
                root.display()
            )]
        })?
        .to_str()
        .ok_or_else(|| vec![format!("{} is not valid UTF-8", out.display())])?
        .to_string();
    match platform {
        Platform::Android => {
            let engine = container_engine()?;
            build_android_image(engine, &root)?;
            let id = start_container(engine, &root, &describe)?;
            let roots = Roots {
                host: root,
                run: CONTAINER_ROOT.to_string(),
            };
            let outcome = execute(
                &Runner::Container {
                    engine,
                    id: id.clone(),
                },
                &android_plan(&roots, &relative_out, &abis),
            );
            workbench::stdout_of(engine, &["rm", "--force", &id])?;
            outcome
        }
        Platform::Ios => {
            if env::consts::OS != MACOS {
                return Err(vec!["iOS packaging requires macOS with Xcode".to_string()]);
            }
            let roots = Roots {
                run: workbench::utf8(&root)?.to_string(),
                host: root,
            };
            execute(&Runner::Host, &ios_plan(&roots, &relative_out, &describe))
        }
    }
    .map(|()| out)
}

/// Parse the platform, the absolute output directory, and the selected Android ABIs.
fn parse(
    args: &[String],
) -> Result<(Platform, path::PathBuf, Vec<&'static AndroidAbi>), Vec<String>> {
    let (selection, flags) = args.split_first().ok_or_else(|| vec![USAGE.to_string()])?;
    let platform = match selection.as_str() {
        "android" => Platform::Android,
        "ios" => Platform::Ios,
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
        None => ANDROID_ABIS.iter().collect(),
        Some(name) => vec![ANDROID_ABIS
            .iter()
            .find(|abi| abi.jni_dir == name)
            .ok_or_else(|| vec![format!("unknown Android ABI `{name}`")])?],
    };
    Ok((platform, absolute_out, abis))
}

/// The first container engine that answers `--version`.
fn container_engine() -> Result<&'static str, Vec<String>> {
    ENGINES
        .into_iter()
        .find(|engine| workbench::stdout_of(engine, &["--version"]).is_ok())
        .ok_or_else(|| vec![format!("none of {} is installed", ENGINES.join(", "))])
}

/// Build the Android tool image from its Dockerfile.
fn build_android_image(engine: &str, root: &path::Path) -> Result<(), Vec<String>> {
    workbench::stdout_of(
        engine,
        &[
            "build",
            "--tag",
            ANDROID_IMAGE,
            "--file",
            workbench::utf8(&root.join(ANDROID_DOCKERFILE))?,
            workbench::utf8(&root.join(ANDROID_CONTEXT))?,
        ],
    )
    .map(drop)
}

/// Start a long-lived container of the Android tool image with zingolib mounted, and return its id.
fn start_container(engine: &str, root: &path::Path, describe: &str) -> Result<String, Vec<String>> {
    let mount = format!("{}:{CONTAINER_ROOT}", workbench::utf8(root)?);
    let describe_env = format!("{DESCRIBE_VARIABLE}={describe}");
    let toolchain_env = format!("{TOOLCHAIN_VARIABLE}={BUILDER_TOOLCHAIN}");
    workbench::stdout_of(
        engine,
        &[
            "run",
            "--detach",
            "--volume",
            &mount,
            "--env",
            &describe_env,
            "--env",
            &toolchain_env,
            ANDROID_IMAGE,
            "sleep",
            "infinity",
        ],
    )
    .map(|id| id.trim().to_string())
}

/// The Android build plan: bindings, per-ABI libraries stripped as zingo-mobile strips them, then copies.
fn android_plan(roots: &Roots, relative_out: &str, abis: &[&AndroidAbi]) -> Vec<Step> {
    let wallet_target = roots.run_path(&format!("{BUILD_ROOT}/android/wallet"));
    let proxy_target = roots.run_path(&format!("{BUILD_ROOT}/android/proxy"));
    let kotlin_out = roots.run_path(&format!("{relative_out}/{KOTLIN_OUT_DIR}"));
    let wallet_library = |abi: &AndroidAbi| {
        format!(
            "{wallet_target}/{}/{PROFILE_DIR}/{}",
            abi.triple,
            binding_layer::library_file(
                LIBRARY_PREFIX,
                binding_layer::WALLET_LIB_NAME,
                SHARED_SUFFIX
            )
        )
    };
    let proxy_file =
        binding_layer::library_file(LIBRARY_PREFIX, binding_layer::PROXY_LIB_NAME, SHARED_SUFFIX);
    let proxy_library =
        |abi: &AndroidAbi| format!("{proxy_target}/{}/{PROFILE_DIR}/{proxy_file}", abi.triple);
    let wallet_crate_dir = roots.run_path(WALLET_CRATE_DIR);
    let proxy_crate_dir = roots.run_path(PROXY_CRATE_DIR);
    let bindgen_inputs = |proxy: &str| {
        (
            format!("{wallet_crate_dir}/{MANIFEST}"),
            format!("{wallet_crate_dir}/{UDL}"),
            roots.run_path(&format!("{WALLET_WORKSPACE_DIR}/{MANIFEST}")),
            proxy.to_string(),
        )
    };
    let bindgen = |generation, proxy: &str, env: Vec<(String, String)>| {
        let (wallet_crate, udl, wallet_workspace, proxy_library) = bindgen_inputs(proxy);
        Step::Run {
            workdir: wallet_crate_dir.clone(),
            env,
            command: [
                vec!["cargo".to_string()],
                binding_layer::bindgen_args(
                    generation,
                    KOTLIN,
                    &binding_layer::BindgenInputs {
                        wallet_crate: &wallet_crate,
                        udl: &udl,
                        wallet_workspace: &wallet_workspace,
                        proxy_library: &proxy_library,
                        target_dir: &wallet_target,
                    },
                    &kotlin_out,
                    binding_layer::Profile::Release,
                ),
            ]
            .concat(),
        }
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
        |abi: &AndroidAbi, workdir: &str, target_dir: &str, package: &[&str]| Step::Run {
            workdir: workdir.to_string(),
            env: abi.env(target_dir),
            command: [
                ["cargo", "ndk", "--target", abi.triple, "build"].as_slice(),
                BUILDER_PROFILE.cargo_args(),
                package,
            ]
            .concat()
            .into_iter()
            .map(String::from)
            .collect(),
        };
    let wallet_steps = abis.iter().flat_map(|abi| {
        [ndk_build(abi, &wallet_crate_dir, &wallet_target, &[])]
            .into_iter()
            .chain(strip(wallet_library(abi), &abi.env(&wallet_target)))
    });
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
                abi.env(&wallet_target),
            )
        });
        [build]
            .into_iter()
            .chain(generate)
            .chain(strip(proxy_library(abi), &abi.env(&proxy_target)))
    });
    let host_copies = abis.iter().flat_map(|abi| {
        let jni = format!("{relative_out}/{JNI_LIBS_DIR}/{}", abi.jni_dir);
        [
            Step::Copy {
                from: host_of(roots, &wallet_library(abi)),
                to: roots.host_path(&format!("{jni}/{ANDROID_WALLET_LIBRARY}")),
            },
            Step::Copy {
                from: host_of(roots, &proxy_library(abi)),
                to: roots.host_path(&format!("{jni}/{proxy_file}")),
            },
        ]
    });
    [
        Step::FreshDir(roots.host_path(relative_out)),
        bindgen(
            binding_layer::Generation::Wallet,
            "",
            vec![(TARGET_DIR_VARIABLE.to_string(), wallet_target.clone())],
        ),
    ]
    .into_iter()
    .chain(wallet_steps)
    .chain(proxy_steps)
    .chain(host_copies)
    .collect()
}

/// The language of the Android bindings.
const KOTLIN: &str = "kotlin";

/// The language of the iOS bindings.
const SWIFT: &str = "swift";

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

/// The iOS build plan, which reproduces zingo-mobile's `build_ios.mjs` into the output directory.
fn ios_plan(roots: &Roots, relative_out: &str, describe: &str) -> Vec<Step> {
    let env = vec![
        (
            "IPHONEOS_DEPLOYMENT_TARGET".to_string(),
            IOS_DEPLOYMENT_TARGET.to_string(),
        ),
        (
            TOOLCHAIN_VARIABLE.to_string(),
            BUILDER_TOOLCHAIN.to_string(),
        ),
        (DESCRIBE_VARIABLE.to_string(), describe.to_string()),
    ];
    let wallet_target = roots.run_path(&format!("{BUILD_ROOT}/ios/wallet"));
    let proxy_target = roots.run_path(&format!("{BUILD_ROOT}/ios/proxy"));
    let with_target = |target_dir: &str| {
        [
            env.clone(),
            vec![(TARGET_DIR_VARIABLE.to_string(), target_dir.to_string())],
        ]
        .concat()
    };
    let wallet_crate_dir = roots.run_path(WALLET_CRATE_DIR);
    let proxy_crate_dir = roots.run_path(PROXY_CRATE_DIR);
    let wallet_workspace_dir = roots.run_path(WALLET_WORKSPACE_DIR);
    let wallet_static = binding_layer::library_file(
        LIBRARY_PREFIX,
        binding_layer::WALLET_LIB_NAME,
        STATIC_SUFFIX,
    );
    let proxy_static =
        binding_layer::library_file(LIBRARY_PREFIX, binding_layer::PROXY_LIB_NAME, STATIC_SUFFIX);
    let library = |target_dir: &str, triple: &str, file: &str| {
        format!("{target_dir}/{triple}/{PROFILE_DIR}/{file}")
    };
    let universal =
        |target_dir: &str, file: &str| library(target_dir, UNIVERSAL_SIMULATOR_DIR, file);
    let wallet_generated = format!("{relative_out}/{GENERATED_DIR}/wallet");
    let proxy_generated = format!("{relative_out}/{GENERATED_DIR}/proxy");
    let headers = format!("{relative_out}/{GENERATED_DIR}/headers");
    let bindgen = |generation, out: &str, workdir: &str| {
        let wallet_crate = format!("{wallet_crate_dir}/{MANIFEST}");
        let udl = format!("{wallet_crate_dir}/{UDL}");
        let wallet_workspace = format!("{wallet_workspace_dir}/{MANIFEST}");
        let proxy_library = library(&proxy_target, IOS_DEVICE_TARGET, &proxy_static);
        Step::Run {
            workdir: workdir.to_string(),
            env: env.clone(),
            command: [
                vec!["cargo".to_string()],
                binding_layer::bindgen_args(
                    generation,
                    SWIFT,
                    &binding_layer::BindgenInputs {
                        wallet_crate: &wallet_crate,
                        udl: &udl,
                        wallet_workspace: &wallet_workspace,
                        proxy_library: &proxy_library,
                        target_dir: &wallet_target,
                    },
                    &roots.run_path(out),
                    binding_layer::Profile::Release,
                ),
            ]
            .concat(),
        }
    };
    let cargo_builds = |workdir: &str, target_dir: &str, package: &[&str]| {
        iter_targets()
            .map(|triple| Step::Run {
                workdir: workdir.to_string(),
                env: with_target(target_dir),
                command: [
                    ["cargo", "build", "--target", triple].as_slice(),
                    BUILDER_PROFILE.cargo_args(),
                    package,
                ]
                .concat()
                .into_iter()
                .map(String::from)
                .collect(),
            })
            .collect::<Vec<_>>()
    };
    let lipo = |target_dir: &str, file: &str| {
        [
            Step::FreshDir(host_of(
                roots,
                &format!("{target_dir}/{UNIVERSAL_SIMULATOR_DIR}/release"),
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
        let output = roots.run_path(&format!("{relative_out}/{name}"));
        let header_args = if with_headers {
            vec!["-headers".to_string(), roots.run_path(&headers)]
        } else {
            vec![]
        };
        [
            Step::Remove(host_of(roots, &output)),
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
    let host = |relative: &str| roots.host_path(relative);
    let steps: Vec<Vec<Step>> = vec![
        vec![
            Step::FreshDir(host(relative_out)),
            Step::FreshDir(host(&wallet_generated)),
            Step::FreshDir(host(&proxy_generated)),
            Step::FreshDir(host(&headers)),
            bindgen(
                binding_layer::Generation::Wallet,
                &wallet_generated,
                &wallet_crate_dir,
            ),
        ],
        cargo_builds(&wallet_crate_dir, &wallet_target, &[]),
        lipo(&wallet_target, &wallet_static).into_iter().collect(),
        cargo_builds(
            &proxy_crate_dir,
            &proxy_target,
            &["-p", binding_layer::PROXY_PACKAGE],
        ),
        vec![bindgen(
            binding_layer::Generation::Proxy,
            &proxy_generated,
            &wallet_workspace_dir,
        )],
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
            WALLET_XCFRAMEWORK,
            vec![
                library(&wallet_target, IOS_DEVICE_TARGET, &wallet_static),
                universal(&wallet_target, &wallet_static),
            ],
            true,
        )
        .into_iter()
        .collect(),
        xcframework(
            PROXY_XCFRAMEWORK,
            vec![
                library(&proxy_target, IOS_DEVICE_TARGET, &proxy_static),
                universal(&proxy_target, &proxy_static),
            ],
            false,
        )
        .into_iter()
        .collect(),
        vec![
            Step::Copy {
                from: host(&format!("{wallet_generated}/{WALLET_SWIFT}")),
                to: host(&format!(
                    "{relative_out}/{SWIFT_SOURCES_DIR}/{WALLET_SWIFT}"
                )),
            },
            Step::Copy {
                from: host(&format!("{proxy_generated}/{PROXY_SWIFT}")),
                to: host(&format!("{relative_out}/{SWIFT_SOURCES_DIR}/{PROXY_SWIFT}")),
            },
        ],
    ];
    steps.into_iter().flatten().collect()
}

/// The device target followed by the simulator targets, in zingo-mobile's order.
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
            create_parent(to)?;
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
            create_parent(to)?;
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

/// Run one command on the runner, streaming its output, and fail if it fails.
fn run_command(
    runner: &Runner,
    workdir: &str,
    env: &[(String, String)],
    command: &[String],
) -> Result<(), Vec<String>> {
    let (program, args) = command
        .split_first()
        .ok_or_else(|| vec!["a plan step has an empty command".to_string()])?;
    let mut process = match runner {
        Runner::Host => {
            let mut host = process::Command::new(program);
            host.args(args)
                .current_dir(workdir)
                .envs(env.iter().cloned());
            host
        }
        Runner::Container { engine, id } => {
            let mut container = process::Command::new(engine);
            container
                .arg("exec")
                .args(["--workdir", workdir])
                .args(
                    env.iter()
                        .flat_map(|(key, value)| ["--env".to_string(), format!("{key}={value}")]),
                )
                .arg(id)
                .arg(program)
                .args(args);
            container
        }
    };
    let status = process
        .status()
        .map_err(|e| vec![format!("cannot run {program}: {e}")])?;
    if status.success() {
        Ok(())
    } else {
        Err(vec![format!("`{}` failed ({status})", command.join(" "))])
    }
}

/// Create the parent directory of a file.
fn create_parent(file: &path::Path) -> Result<(), Vec<String>> {
    file.parent().map_or(Ok(()), |parent| {
        fs::create_dir_all(parent)
            .map_err(|e| vec![format!("cannot create {}: {e}", parent.display())])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of times one build generates the proxy bindings.
    const PROXY_GENERATIONS_PER_BUILD: usize = 1;

    fn roots() -> Roots {
        Roots {
            host: path::PathBuf::from("/host/zingolib"),
            run: CONTAINER_ROOT.to_string(),
        }
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
    fn android_cc_carries_the_api_level() {
        assert_eq!(
            ANDROID_ABIS[FIRST_POSITION].cc(),
            "aarch64-linux-android26-clang"
        );
    }

    #[test]
    fn android_plan_generates_the_proxy_bindings_once_after_the_first_abi() {
        let abis: Vec<&AndroidAbi> = ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots(), "bindings/android/build/binding-layer", &abis);
        let generations = commands(&plan)
            .into_iter()
            .filter(|command| command.contains("--library"))
            .count();
        assert_eq!(generations, PROXY_GENERATIONS_PER_BUILD);
    }

    #[test]
    fn android_plan_copies_both_libraries_for_every_abi() {
        let abis: Vec<&AndroidAbi> = ANDROID_ABIS.iter().collect();
        let plan = android_plan(&roots(), "out", &abis);
        let copies = plan
            .iter()
            .filter(|step| matches!(step, Step::Copy { .. }))
            .count();
        assert_eq!(
            copies,
            ANDROID_ABIS.len() * binding_layer::GENERATIONS.len()
        );
    }

    #[test]
    fn host_of_maps_container_paths_back_to_the_host() {
        assert_eq!(
            host_of(&roots(), "/opt/zingolib/target/x.so"),
            path::PathBuf::from("/host/zingolib/target/x.so")
        );
    }
}
