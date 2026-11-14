use crate::binding_manifest;
use std::path;

#[path = "../../../crates/zingolib/short_commit.rs"]
pub mod short_commit;

/// The language of the Android bindings.
pub const KOTLIN: &str = "kotlin";

/// The language of the iOS bindings.
pub const SWIFT: &str = "swift";

/// The binding languages that the builder generates.
pub const LANGUAGES: [&str; 2] = [KOTLIN, SWIFT];

const BINDGEN_PACKAGE: &str = "zingolib-uniffi-bindgen";

const BINDGEN_BIN: &str = "zingolib-uniffi-bindgen";

/// The wallet crate's library name, from which its library file names derive.
pub const WALLET_LIB_NAME: &str = "zingo";

/// The proxy crate's library name, from which its library file names derive.
pub const PROXY_LIB_NAME: &str = "zingo_nym_proxy_ffi";

const MACH_O_SYMBOL_PREFIX: &str = "_";
const UNIFFI_SYMBOL_PREFIXES: [&str; 2] = ["uniffi_", "ffi_"];
const SYMBOL_WILDCARD: &str = "*";
const SYMBOL_SEPARATOR: &str = "_";

pub fn exported_symbol_patterns(lib_name: &str) -> [String; 2] {
    UNIFFI_SYMBOL_PREFIXES.map(|prefix| {
        format!("{MACH_O_SYMBOL_PREFIX}{prefix}{lib_name}{SYMBOL_SEPARATOR}{SYMBOL_WILDCARD}")
    })
}

/// The proxy crate's package name, which cargo selects in its own workspace.
pub const PROXY_PACKAGE: &str = "zingolib-nym-proxy-ffi";

pub const PROXY_FEATURE_ARGS: [&str; 2] = ["--features", PROXY_PACKAGE];

pub const WALLET_CRATE_DIR: &str = "ffi/lib";

pub const PROXY_CRATE_DIR: &str = "crates/netutils/nym-proxy-ffi";

pub const BINDING_CRATE_DIRS: [&str; 2] = [WALLET_CRATE_DIR, PROXY_CRATE_DIR];

pub const TOOLCHAIN_VARIABLE: &str = "RUSTUP_TOOLCHAIN";

pub const IMAGE_TOOLCHAIN_ARGUMENT: &str = "RUST_TOOLCHAIN_TOML";

pub const IMAGE_TARGETS_ARGUMENT: &str = "RUST_TARGETS";

/// The variable that tells cargo where to write build output.
pub const TARGET_DIR_VARIABLE: &str = "CARGO_TARGET_DIR";

pub const DESCRIPTOR_FILE: &str = "descriptor.txt";

pub const DESCRIPTOR_ENV: &str = "ZINGOLIB_DESCRIPTOR";
pub const MESSAGE_FORMAT_FLAG: &str = "--message-format=json-render-diagnostics";
const ZINGOLIB_PACKAGE_ID_TAIL: &str = "/zingolib#";
const JSON_STRING_END: char = '"';
const JSON_ESCAPE: char = '\\';

pub const NDK_ENV_COMMAND: [&str; 2] = ["cargo", "ndk-env"];
const NDK_ENV_TARGET_FLAG: &str = "--target";
const NDK_ENV_JSON_FLAG: &str = "--json";
pub const NDK_CLANG_PATH_VARIABLE: &str = "CLANG_PATH";
pub const NDK_LINK_CLANG_VARIABLE: &str = "_CARGO_NDK_LINK_CLANG";
pub const NDK_LINK_TARGET_VARIABLE: &str = "_CARGO_NDK_LINK_TARGET";
const CLANG_TARGET_FLAG: &str = "--target=";
const CLANG_SUFFIX: &str = "-clang";

/// The container engines to try, in order.
pub const ENGINES: [&str; 2] = ["podman", "docker"];
pub const ENGINE_VARIABLE: &str = "CONTAINER_RUNTIME";

pub const PUBLISHED_ANDROID_IMAGE: &str = "ghcr.io/zingolabs/android_builder:019";

pub const IOS_XCODE_VERSION: &str = "26.3";

/// The Android API level that the builder compiles against.
pub const ANDROID_API_LEVEL: &str = "26";

/// The Android output subdirectory that holds the Kotlin sources the AAR compiles.
pub const KOTLIN_OUT_DIR: &str = "kotlin";

/// The Android output subdirectory that holds the per-ABI libraries the AAR packages.
pub const JNI_LIBS_DIR: &str = "jniLibs";

/// The name of the wallet's Android library.
pub const ANDROID_WALLET_LIBRARY: &str = "libuniffi_zingo.so";

/// The prefix of a Unix library file name.
pub const LIBRARY_PREFIX: &str = "lib";

/// The suffix of an Android shared library.
pub const SHARED_SUFFIX: &str = ".so";

/// The suffix of an iOS static library.
pub const STATIC_SUFFIX: &str = ".a";

/// The operating system name that Rust reports on macOS.
pub const MACOS: &str = "macos";

pub const WALLET_BINARY_TARGET: &str = "Zingolib";

#[cfg(test)]
const XCFRAMEWORK_SUFFIX: &str = ".xcframework";

pub const WALLET_XCFRAMEWORK: &str = "Zingolib.xcframework";

const SWIFT_SOURCES_PARENT: &str = "Sources";

pub const SWIFT_PACKAGE: &str = "ZingoBindings";

pub fn swift_sources_dir() -> String {
    format!("{SWIFT_SOURCES_PARENT}/{SWIFT_PACKAGE}")
}

pub const SWIFT_PACKAGE_MANIFEST: &str = "bindings/swift/Package.swift";
pub const SWIFT_LINKED_FRAMEWORK: &str = "SystemConfiguration";

pub const SWIFT_PACKAGE_OUTPUT_DIR: &str = "build";

pub const SWIFT_CONTRACT_TESTS_DIR: &str = "crates/netutils/nym-proxy-ffi/contract-tests";

pub const SWIFT_CONTRACT_TESTS_PACKAGE: &str = "ZingoBindingsContractTests";

pub const SWIFT_PACKAGE_FROM_CONTRACT_TESTS: &str = "../../../../bindings/swift";

const SWIFTPM_PACKAGE_SCHEME_SUFFIX: &str = "-Package";

pub fn swift_contract_tests_scheme() -> String {
    format!("{SWIFT_CONTRACT_TESTS_PACKAGE}{SWIFTPM_PACKAGE_SCHEME_SUFFIX}")
}

const SIMCTL_RUNTIME_PREFIX: &str = "-- ";

const SIMCTL_RUNTIME_SUFFIX: &str = " --";

const SIMCTL_IOS_RUNTIME: &str = "iOS ";

const SIMCTL_IPHONE: &str = "iPhone";

const SIMCTL_FIELD_OPEN: &str = " (";

const SIMCTL_FIELD_CLOSE: &str = ")";

const SIMCTL_BOOTED: &str = "Booted";

const SIMCTL_DEVICE_FIELDS: usize = 3;

const VERSION_SEPARATOR: char = '.';

pub const SWIFT_CONTRACT_TEST_COUNT: usize = 4;

const XCODEBUILD_EXECUTED: &str = "Executed ";

#[derive(Debug, PartialEq, Eq)]
pub struct IosSimulator {
    pub name: String,
    pub udid: String,
}

fn version(text: &str) -> Option<Vec<u32>> {
    text.split(VERSION_SEPARATOR)
        .map(|part| part.parse().ok())
        .collect()
}

pub fn ios_simulator(listing: &str, minimum_runtime: &str) -> Result<IosSimulator, Vec<String>> {
    let floor = version(minimum_runtime)
        .ok_or_else(|| vec![format!("`{minimum_runtime}` is not a dotted version")])?;
    let mut eligible = false;
    let mut first = None;
    for line in listing.lines() {
        if let Some(runtime) = line
            .strip_prefix(SIMCTL_RUNTIME_PREFIX)
            .and_then(|rest| rest.strip_suffix(SIMCTL_RUNTIME_SUFFIX))
        {
            eligible = runtime
                .strip_prefix(SIMCTL_IOS_RUNTIME)
                .and_then(version)
                .is_some_and(|found| found >= floor);
            continue;
        }
        if !eligible {
            continue;
        }
        let fields: Vec<&str> = line
            .trim()
            .rsplitn(SIMCTL_DEVICE_FIELDS, SIMCTL_FIELD_OPEN)
            .collect();
        let [state, udid, name] = fields[..] else {
            continue;
        };
        if !name.starts_with(SIMCTL_IPHONE) {
            continue;
        }
        let simulator = IosSimulator {
            name: name.to_string(),
            udid: udid.trim_end_matches(SIMCTL_FIELD_CLOSE).to_string(),
        };
        if state.trim_end_matches(SIMCTL_FIELD_CLOSE) == SIMCTL_BOOTED {
            return Ok(simulator);
        }
        first.get_or_insert(simulator);
    }
    first.ok_or_else(|| {
        vec![format!(
            "no available {SIMCTL_IPHONE} simulator on an {SIMCTL_IOS_RUNTIME}{minimum_runtime} or newer runtime in:\n{listing}"
        )]
    })
}

pub fn executed_tests(log: &str) -> Result<usize, Vec<String>> {
    log.lines()
        .filter_map(|line| line.trim().strip_prefix(XCODEBUILD_EXECUTED))
        .filter_map(|rest| rest.split_whitespace().next()?.parse().ok())
        .next_back()
        .ok_or_else(|| {
            vec![format!(
                "xcodebuild reported no `{XCODEBUILD_EXECUTED}N tests` line"
            )]
        })
}

/// The wallet's generated Swift source.
pub const WALLET_SWIFT: &str = "zingo.swift";

/// The proxy's generated Swift source.
pub const PROXY_SWIFT: &str = "zingo_nym_proxy_ffi.swift";

/// Both generated Swift sources.
pub const SWIFT_SOURCES: [&str; 2] = [WALLET_SWIFT, PROXY_SWIFT];

/// One Android ABI, with the environment that the builder sets for it.
pub struct AndroidAbi {
    /// The Rust target triple.
    pub triple: &'static str,
    /// The directory name under `jniLibs`.
    pub jni_dir: &'static str,
    /// The prefix of the NDK clang wrapper, before the API level.
    pub clang_prefix: &'static str,
    /// The value of `CARGO_FEATURE_STD` that the builder sets.
    pub std_feature: &'static str,
}

pub struct NdkLink {
    pub env_command: Vec<String>,
    pub link_target: String,
}

impl AndroidAbi {
    pub fn clang_target(&self) -> String {
        format!("{}{ANDROID_API_LEVEL}", self.clang_prefix)
    }

    /// The NDK clang wrapper for this ABI at the builder's API level.
    pub fn cc(&self) -> String {
        format!("{}{CLANG_SUFFIX}", self.clang_target())
    }

    pub fn ndk_link(&self) -> NdkLink {
        NdkLink {
            env_command: ndk_env_command(self.triple),
            link_target: format!("{CLANG_TARGET_FLAG}{}", self.clang_target()),
        }
    }

    /// The environment that the builder sets while it builds this ABI.
    pub fn env(&self, target_dir: &str) -> Vec<(String, String)> {
        [
            ("CARGO_FEATURE_STD", self.std_feature.to_string()),
            ("CC", self.cc()),
            (TARGET_DIR_VARIABLE, target_dir.to_string()),
        ]
        .map(|(key, value)| (key.to_string(), value))
        .to_vec()
    }
}

/// Every Android ABI, in the order that the builder builds them.
pub const ANDROID_ABIS: [AndroidAbi; 4] = [
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

pub const ARTIFACT_PREFIX: &str = "binding-layer";
pub const BUNDLE_ARTIFACT: &str = "bundle";
pub const ABI_ARTIFACT: &str = "abi";
pub const ARTIFACT_KINDS: [&str; 2] = [BUNDLE_ARTIFACT, ABI_ARTIFACT];
pub const ARTIFACT_WILDCARD: &str = "*";
const ARTIFACT_SEPARATOR: &str = "-";
const SLICE_SEGMENT: &str = "slice";
pub const ARCHIVE_SUFFIX: &str = ".tar.gz";
pub const WALLET_MANIFEST: &str = "crates/zingolib/Cargo.toml";
const MANIFEST_VERSION_PATH: [&str; 2] = ["package", "version"];
pub const ANDROID_BUILD_DIR: &str = "bindings/android/build";
pub const MAVEN_REPOSITORY_DIR: &str = "repository";
pub const MAVEN_GROUP: &str = "org.zingolabs";
pub const MAVEN_ARTIFACT: &str = "zingo-binding-layer";
pub const MAVEN_VERSION_PROPERTY: &str = "bindingLayerVersion";
pub const MAVEN_METADATA_FILE: &str = "maven-metadata.xml";
pub const MAVEN_PUBLISH_PLUGIN: &str = "maven-publish";
pub const MAVEN_COMMIT_LENGTH: usize = short_commit::LENGTH;
const MAVEN_VERSION_SEPARATOR: &str = "-";
const MAVEN_COMMIT_KEY: &str = "commit";
pub const ABIS_OUTPUT: &str = "abis";
pub const ABI_ARTIFACTS_OUTPUT: &str = "abi_artifacts";
pub const ABI_PATTERN_OUTPUT: &str = "abi_pattern";
pub const ANDROID_REPOSITORY_OUTPUT: &str = "android_repository";
pub const MAVEN_VERSION_OUTPUT: &str = "maven_version";
pub const BUNDLE_OUTPUT_PREFIX: &str = "bundle_";
pub const BUNDLE_PATTERN_OUTPUT: &str = "bundle_pattern";
pub const IOS_OUT_OUTPUT: &str = "ios_out";
const OUTPUT_ASSIGNMENT: char = '=';
const JSON_QUOTE: char = '"';
const JSON_SEPARATOR: &str = ",";
const JSON_PAIR: char = ':';
const JSON_OBJECT_OPEN: char = '{';
const UNPARSED_MESSAGE: &str = "a cargo message does not match the cargo_metadata schema: ";

pub fn descriptor_in_messages(messages: &str) -> Result<Option<String>, Vec<String>> {
    let mut unparsed = Vec::new();
    for message in cargo_metadata::Message::parse_stream(messages.as_bytes()) {
        match message {
            Ok(cargo_metadata::Message::BuildScriptExecuted(script))
                if script.package_id.repr.contains(ZINGOLIB_PACKAGE_ID_TAIL) =>
            {
                let descriptor = script
                    .env
                    .into_iter()
                    .find(|(key, _)| key == DESCRIPTOR_ENV)
                    .map(|(_, value)| value)
                    .filter(|descriptor| !descriptor.is_empty());
                if descriptor.is_some() {
                    return Ok(descriptor);
                }
            }
            Ok(cargo_metadata::Message::TextLine(line)) if line.starts_with(JSON_OBJECT_OPEN) => {
                unparsed.push(format!("{UNPARSED_MESSAGE}{line}"));
            }
            Ok(_) => {}
            Err(e) => unparsed.push(format!("{UNPARSED_MESSAGE}{e}")),
        }
    }
    if unparsed.is_empty() {
        Ok(None)
    } else {
        Err(unparsed)
    }
}

pub fn ndk_env_command(triple: &str) -> Vec<String> {
    [
        NDK_ENV_COMMAND.as_slice(),
        &[NDK_ENV_TARGET_FLAG, triple, NDK_ENV_JSON_FLAG],
    ]
    .concat()
    .into_iter()
    .map(String::from)
    .collect()
}

fn json_string_prefix(text: &str) -> Option<(String, &str)> {
    let body = text.strip_prefix(JSON_QUOTE)?;
    let mut chars = body.char_indices();
    let mut value = String::new();
    while let Some((at, ch)) = chars.next() {
        match ch {
            JSON_STRING_END => return Some((value, &body[at + JSON_STRING_END.len_utf8()..])),
            JSON_ESCAPE => value.push(match chars.next()?.1 {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                'u' => return None,
                other => other,
            }),
            other => value.push(other),
        }
    }
    None
}

pub fn env_in_json(text: &str) -> Result<Vec<(String, String)>, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !matches!(*line, "" | "{" | "}"))
        .map(|line| {
            let pair = line.strip_suffix(JSON_SEPARATOR).unwrap_or(line);
            let parsed = json_string_prefix(pair).and_then(|(key, rest)| {
                let rest = rest.trim_start().strip_prefix(JSON_PAIR)?.trim_start();
                let (value, rest) = json_string_prefix(rest)?;
                rest.is_empty().then_some((key, value))
            });
            parsed.ok_or_else(|| format!("not a JSON string pair: {line}"))
        })
        .collect()
}

pub fn ndk_link_env(
    exported: Vec<(String, String)>,
    link_target: &str,
) -> Result<Vec<(String, String)>, String> {
    let clang = exported
        .iter()
        .find(|(key, _)| key == NDK_CLANG_PATH_VARIABLE)
        .map(|(_, value)| value.clone())
        .ok_or_else(|| {
            format!(
                "`{}` exported no {NDK_CLANG_PATH_VARIABLE}",
                NDK_ENV_COMMAND.join(" ")
            )
        })?;
    let mut env = exported;
    env.push((NDK_LINK_CLANG_VARIABLE.to_string(), clang));
    env.push((
        NDK_LINK_TARGET_VARIABLE.to_string(),
        link_target.to_string(),
    ));
    Ok(env)
}

pub fn artifact_name(kind: &str, segment: &str, commit: &str) -> String {
    [ARTIFACT_PREFIX, kind, segment, commit].join(ARTIFACT_SEPARATOR)
}

pub fn artifact_name_without_commit(segment: &str) -> String {
    [ARTIFACT_PREFIX, segment].join(ARTIFACT_SEPARATOR)
}

pub fn ios_slice_artifact(bundle: &str, triple: &str) -> String {
    [bundle, SLICE_SEGMENT, triple].join(ARTIFACT_SEPARATOR)
}

pub fn ios_slice_pattern(bundle: &str) -> String {
    ios_slice_artifact(bundle, ARTIFACT_WILDCARD)
}

fn json_string(text: &str) -> String {
    format!("{JSON_QUOTE}{text}{JSON_QUOTE}")
}

fn json_list(items: impl Iterator<Item = String>) -> String {
    format!("[{}]", items.collect::<Vec<_>>().join(JSON_SEPARATOR))
}

fn json_object(pairs: impl Iterator<Item = (String, String)>) -> String {
    format!(
        "{{{}}}",
        pairs
            .map(|(key, value)| format!("{}{JSON_PAIR}{}", json_string(&key), json_string(&value)))
            .collect::<Vec<_>>()
            .join(JSON_SEPARATOR)
    )
}

pub fn swift_package_output_dir() -> String {
    let manifest = path::Path::new(SWIFT_PACKAGE_MANIFEST);
    let dir = manifest.parent().unwrap_or(manifest);
    format!("{}/{SWIFT_PACKAGE_OUTPUT_DIR}", dir.display())
}

pub fn maven_repository_dir() -> String {
    format!("{ANDROID_BUILD_DIR}/{MAVEN_REPOSITORY_DIR}")
}

pub fn crate_version(manifest: &str) -> Result<String, Vec<String>> {
    let document: toml_edit::DocumentMut = manifest
        .parse()
        .map_err(|error| vec![format!("{WALLET_MANIFEST} does not parse: {error}")])?;
    MANIFEST_VERSION_PATH
        .iter()
        .try_fold(document.as_item(), |item, key| item.get(key))
        .and_then(toml_edit::Item::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            vec![format!(
                "{WALLET_MANIFEST} names no string at {}",
                MANIFEST_VERSION_PATH.join(".")
            )]
        })
}

pub fn maven_version(crate_version: &str, commit: &str) -> Result<String, Vec<String>> {
    let commit = binding_manifest::commit_value(MAVEN_COMMIT_KEY, commit.trim())?;
    let short = short_commit::abbreviate(&commit).ok_or_else(|| {
        vec![format!(
            "{MAVEN_COMMIT_KEY} {commit} has no {MAVEN_COMMIT_LENGTH}-character abbreviation"
        )]
    })?;
    Ok(format!("{crate_version}{MAVEN_VERSION_SEPARATOR}{short}"))
}

pub fn artifact_outputs(
    platforms: &[&str],
    commit: &str,
    crate_version: &str,
) -> Result<Vec<(String, String)>, Vec<String>> {
    let abis = ANDROID_ABIS.iter().map(|abi| abi.jni_dir);
    let mut outputs = vec![
        (
            ABIS_OUTPUT.to_string(),
            json_list(abis.clone().map(json_string)),
        ),
        (
            ABI_ARTIFACTS_OUTPUT.to_string(),
            json_object(
                abis.map(|abi| (abi.to_string(), artifact_name(ABI_ARTIFACT, abi, commit))),
            ),
        ),
        (
            ABI_PATTERN_OUTPUT.to_string(),
            artifact_name(ABI_ARTIFACT, ARTIFACT_WILDCARD, commit),
        ),
        (
            ANDROID_REPOSITORY_OUTPUT.to_string(),
            maven_repository_dir(),
        ),
        (
            MAVEN_VERSION_OUTPUT.to_string(),
            maven_version(crate_version, commit)?,
        ),
    ];
    outputs.extend(platforms.iter().map(|platform| {
        (
            format!("{BUNDLE_OUTPUT_PREFIX}{platform}"),
            artifact_name(BUNDLE_ARTIFACT, platform, commit),
        )
    }));
    outputs.push((
        BUNDLE_PATTERN_OUTPUT.to_string(),
        artifact_name(BUNDLE_ARTIFACT, ARTIFACT_WILDCARD, commit),
    ));
    outputs.push((IOS_OUT_OUTPUT.to_string(), swift_package_output_dir()));
    Ok(outputs)
}

pub fn render_outputs(outputs: &[(String, String)]) -> String {
    outputs
        .iter()
        .map(|(key, value)| format!("{key}{OUTPUT_ASSIGNMENT}{value}\n"))
        .collect()
}

pub fn artifact_segment<'a>(name: &'a str, kind: &str, commit: &str) -> Option<&'a str> {
    let head = [ARTIFACT_PREFIX, kind, ""].join(ARTIFACT_SEPARATOR);
    let tail = ["", commit].join(ARTIFACT_SEPARATOR);
    let segment = name
        .strip_prefix(head.as_str())?
        .strip_suffix(tail.as_str())?;
    (!segment.is_empty()).then_some(segment)
}

/// - Reads `CONTAINER_RUNTIME` from the environment.
/// - Runs `<engine> --version` as a child process for each engine in turn until one answers.
pub fn container_engine() -> Result<&'static str, Vec<String>> {
    if let Ok(named) = std::env::var(ENGINE_VARIABLE) {
        return ENGINES
            .into_iter()
            .find(|engine| *engine == named)
            .ok_or_else(|| {
                vec![format!(
                    "{ENGINE_VARIABLE}={named} names none of {}",
                    ENGINES.join(", ")
                )]
            });
    }
    ENGINES
        .into_iter()
        .find(|engine| crate::stdout_of(engine, &["--version"]).is_ok())
        .ok_or_else(|| vec![format!("none of {} is installed", ENGINES.join(", "))])
}

/// The binding set that the bindgen generates for one crate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Generation {
    Wallet,
    /// The proxy crate's bindings, generated from its built library.
    Proxy,
}

pub struct GenerationNames {
    pub label: &'static str,
    pub lib_name: &'static str,
    pub crate_dir: &'static str,
}

const WALLET_NAMES: GenerationNames = GenerationNames {
    label: "wallet",
    lib_name: WALLET_LIB_NAME,
    crate_dir: WALLET_CRATE_DIR,
};

const PROXY_NAMES: GenerationNames = GenerationNames {
    label: "proxy",
    lib_name: PROXY_LIB_NAME,
    crate_dir: PROXY_CRATE_DIR,
};

impl Generation {
    pub fn names(self) -> &'static GenerationNames {
        match self {
            Generation::Wallet => &WALLET_NAMES,
            Generation::Proxy => &PROXY_NAMES,
        }
    }
}

/// Every binding set, in a fixed order.
pub const GENERATIONS: [Generation; 2] = [Generation::Wallet, Generation::Proxy];

/// The cargo profile that a bindgen run or a library build uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// Cargo's default profile, which writes to the `debug` directory.
    Debug,
    /// Cargo's `--release` profile, which writes to the `release` directory.
    Release,
    Mobile,
}

impl Profile {
    /// The cargo arguments that select this profile.
    pub const fn cargo_args(self) -> &'static [&'static str] {
        match self {
            Profile::Debug => &[],
            Profile::Release => &["--release"],
            Profile::Mobile => &["--profile", "mobile"],
        }
    }

    /// The directory under a target directory that this profile writes to.
    pub const fn directory(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
            Profile::Mobile => "mobile",
        }
    }
}

pub struct BindgenInputs<'a> {
    pub language: &'a str,
    pub profile: Profile,
    /// The wallet-side workspace manifest.
    pub wallet_workspace: &'a str,
    /// The cargo target directory for the bindgen build.
    pub target_dir: &'a str,
}

/// Every pairing of a binding set with a language, in a fixed order.
pub fn binding_sets() -> impl Iterator<Item = (Generation, &'static str)> {
    GENERATIONS
        .into_iter()
        .flat_map(|generation| LANGUAGES.map(|language| (generation, language)))
}

/// The name of the directory that holds one binding set in one language.
pub fn output_name(generation: Generation, language: &str) -> String {
    format!("{}-{language}", generation.names().label)
}

pub fn bindgen_args(inputs: &BindgenInputs, library: &str, out_dir: &str) -> Vec<String> {
    [
        &["run", "--locked", "--target-dir", inputs.target_dir],
        inputs.profile.cargo_args(),
        &[
            crate::MANIFEST_PATH_FLAG,
            inputs.wallet_workspace,
            "--package",
            BINDGEN_PACKAGE,
            "--bin",
            BINDGEN_BIN,
            "--",
            "generate",
            "--library",
            library,
            "--language",
            inputs.language,
            "--out-dir",
            out_dir,
        ],
    ]
    .concat()
    .into_iter()
    .map(String::from)
    .collect()
}

pub fn host_library(lib_name: &str, target_dir: &str, profile: Profile) -> String {
    format!(
        "{target_dir}/{}/{}",
        profile.directory(),
        host_library_file(lib_name)
    )
}

pub fn host_library_file(lib_name: &str) -> String {
    library_file(
        std::env::consts::DLL_PREFIX,
        lib_name,
        std::env::consts::DLL_SUFFIX,
    )
}

/// The file name of a library with the given name, prefix, and suffix.
pub fn library_file(prefix: &str, lib_name: &str, suffix: &str) -> String {
    format!("{prefix}{lib_name}{suffix}")
}

#[cfg(test)]
mod tests;
