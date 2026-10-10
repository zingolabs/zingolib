/// The language of the Android bindings.
pub const KOTLIN: &str = "kotlin";

/// The language of the iOS bindings.
pub const SWIFT: &str = "swift";

/// The binding languages that the builder generates.
pub const LANGUAGES: [&str; 2] = [KOTLIN, SWIFT];

const BINDGEN_PACKAGE: &str = "zingo-uniffi-bindgen";

const BINDGEN_BIN: &str = "zingo-uniffi-bindgen";

/// The wallet crate's library name, from which its library file names derive.
pub const WALLET_LIB_NAME: &str = "zingo";

/// The proxy crate's library name, from which its library file names derive.
pub const PROXY_LIB_NAME: &str = "zingo_nym_proxy_ffi";

/// The proxy crate's package name, which cargo selects in its own workspace.
pub const PROXY_PACKAGE: &str = "zingo-nym-proxy-ffi";

pub const WALLET_CRATE_DIR: &str = "zingo-ffi/lib";

pub const PROXY_CRATE_DIR: &str = "zingo-netutils/nym-proxy-ffi";

pub const BINDING_CRATE_DIRS: [&str; 2] = [WALLET_CRATE_DIR, PROXY_CRATE_DIR];

pub const TOOLCHAIN_VARIABLE: &str = "RUSTUP_TOOLCHAIN";

pub const IMAGE_TOOLCHAIN_ARGUMENT: &str = "RUST_TOOLCHAIN_TOML";

pub const IMAGE_TARGETS_ARGUMENT: &str = "RUST_TARGETS";

/// The variable that tells cargo where to write build output.
pub const TARGET_DIR_VARIABLE: &str = "CARGO_TARGET_DIR";

pub const DESCRIPTOR_FILE: &str = "descriptor.txt";

pub const DESCRIPTOR_ENV: &str = "ZINGOLIB_DESCRIPTOR";
pub const MESSAGE_FORMAT_FLAG: &str = "--message-format=json-render-diagnostics";
const BUILD_SCRIPT_MESSAGE: &str = "\"reason\":\"build-script-executed\"";
const PACKAGE_ID_KEY: &str = "\"package_id\":\"";
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

/// The wallet XCFramework.
pub const WALLET_XCFRAMEWORK: &str = "Zingolib.xcframework";

/// The proxy XCFramework.
pub const PROXY_XCFRAMEWORK: &str = "ZingoNymProxyFFI.xcframework";

/// Both XCFrameworks, in the order that the builder creates them.
pub const XCFRAMEWORKS: [&str; 2] = [WALLET_XCFRAMEWORK, PROXY_XCFRAMEWORK];

const SWIFT_SOURCES_PARENT: &str = "Sources";

pub const SWIFT_PACKAGE: &str = "ZingoBindings";

pub fn swift_sources_dir() -> String {
    format!("{SWIFT_SOURCES_PARENT}/{SWIFT_PACKAGE}")
}

pub const SWIFT_PACKAGE_MANIFEST: &str = "bindings/swift/Package.swift";

pub const SWIFT_PACKAGE_OUTPUT_DIR: &str = "build";

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
pub const AAR_SUFFIX: &str = "-release.aar";
pub const ABIS_OUTPUT: &str = "abis";
pub const ABI_ARTIFACTS_OUTPUT: &str = "abi_artifacts";
pub const ABI_PATTERN_OUTPUT: &str = "abi_pattern";
pub const AAR_GLOB_OUTPUT: &str = "aar_glob";
pub const BUNDLE_OUTPUT_PREFIX: &str = "bundle_";
pub const BUNDLE_PATTERN_OUTPUT: &str = "bundle_pattern";
pub const IOS_OUT_OUTPUT: &str = "ios_out";
const OUTPUT_ASSIGNMENT: char = '=';
const JSON_QUOTE: char = '"';
const JSON_SEPARATOR: &str = ",";
const JSON_PAIR: char = ':';

fn json_string_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let start = text.find(key)? + key.len();
    let rest = &text[start..];
    let end = rest.find(JSON_STRING_END)?;
    Some(&rest[..end])
}

pub fn descriptor_in_messages(messages: &str) -> Option<String> {
    let env_key = format!("[\"{DESCRIPTOR_ENV}\",\"");
    messages
        .lines()
        .filter(|line| line.contains(BUILD_SCRIPT_MESSAGE))
        .filter(|line| {
            json_string_after(line, PACKAGE_ID_KEY)
                .is_some_and(|id| id.contains(ZINGOLIB_PACKAGE_ID_TAIL))
        })
        .find_map(|line| json_string_after(line, &env_key))
        .filter(|descriptor| !descriptor.is_empty())
        .map(str::to_string)
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
    let manifest = std::path::Path::new(SWIFT_PACKAGE_MANIFEST);
    let dir = manifest.parent().unwrap_or(manifest);
    format!("{}/{SWIFT_PACKAGE_OUTPUT_DIR}", dir.display())
}

pub fn artifact_outputs(platforms: &[&str], commit: &str) -> Vec<(String, String)> {
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
            AAR_GLOB_OUTPUT.to_string(),
            format!("{ARTIFACT_WILDCARD}{AAR_SUFFIX}"),
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
    outputs
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

impl Generation {
    /// The prefix of this binding set's output directory names.
    pub fn label(self) -> &'static str {
        match self {
            Generation::Wallet => "wallet",
            Generation::Proxy => "proxy",
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
    /// The `mobile` profile of the root workspace, which the builder ships.
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

/// A directory that a bindgen run starts in, which decides whose `uniffi.toml` library mode applies.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Workdir {
    WalletCrate,
    /// The wallet-side workspace's directory, where library mode sees no proxy configuration.
    WalletWorkspace,
    /// The proxy crate's directory, where library mode applies the proxy's configuration.
    ProxyCrate,
}

/// The directory that the builder generates one binding set in one language from.
pub fn bindgen_workdir(generation: Generation, language: &str) -> Workdir {
    match (generation, language) {
        (Generation::Wallet, _) => Workdir::WalletCrate,
        (Generation::Proxy, KOTLIN) => Workdir::ProxyCrate,
        (Generation::Proxy, _) => Workdir::WalletWorkspace,
    }
}

/// The name of the directory that holds one binding set in one language.
pub fn output_name(generation: Generation, language: &str) -> String {
    format!("{}-{language}", generation.label())
}

pub fn bindgen_args(inputs: &BindgenInputs, library: &str, out_dir: &str) -> Vec<String> {
    [
        &["run", "--locked", "--target-dir", inputs.target_dir],
        inputs.profile.cargo_args(),
        &[
            "--manifest-path",
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
mod tests {
    use super::*;

    #[test]
    fn the_android_workflow_runs_in_the_published_image() {
        let workflow = include_str!("../../../.github/workflows/binding-layer-android.yaml");
        assert!(workflow.contains(&format!("image: {PUBLISHED_ANDROID_IMAGE}\n")));
    }

    #[test]
    fn the_descriptor_env_is_the_one_the_build_script_emits() {
        let build_script = include_str!("../../../zingolib/build.rs");
        assert!(build_script.contains(&format!(
            "const DESCRIPTOR_ENV: &str = \"{DESCRIPTOR_ENV}\";"
        )));
        assert!(build_script.contains("cargo:rustc-env={DESCRIPTOR_ENV}={description}"));
    }

    #[test]
    fn the_descriptor_comes_from_zingolibs_build_script_message_alone() {
        let messages = "\
            {\"reason\":\"compiler-artifact\",\"package_id\":\"path+file:///x/zingolib#6.0.0\"}\n\
            {\"reason\":\"build-script-executed\",\"package_id\":\"path+file:///x/zingolib_testutils#0.1.0\",\"env\":[[\"ZINGOLIB_DESCRIPTOR\",\"wrong\"]],\"out_dir\":\"/x/out\"}\n\
            {\"reason\":\"build-script-executed\",\"package_id\":\"path+file:///x/zingolib#6.0.0\",\"linked_libs\":[],\"env\":[[\"OTHER\",\"1\"],[\"ZINGOLIB_DESCRIPTOR\",\"zl_6.0.0_2691e\"]],\"out_dir\":\"/x/out\"}\n\
            {\"reason\":\"build-finished\",\"success\":true}\n";
        assert_eq!(
            descriptor_in_messages(messages),
            Some("zl_6.0.0_2691e".to_string())
        );
        assert_eq!(
            descriptor_in_messages("{\"reason\":\"build-finished\"}\n"),
            None
        );
        assert_eq!(
            descriptor_in_messages("{\"reason\":\"build-script-executed\",\"package_id\":\"path+file:///x/zingolib#6.0.0\",\"env\":[]}\n"),
            None
        );
    }

    #[test]
    fn the_ndk_env_command_asks_cargo_ndk_for_one_targets_environment_as_json() {
        assert_eq!(
            ndk_env_command("x86_64-linux-android"),
            [
                "cargo",
                "ndk-env",
                "--target",
                "x86_64-linux-android",
                "--json"
            ]
        );
    }

    #[test]
    fn the_exported_environment_is_read_from_cargo_ndks_pretty_json() {
        let json = "{\n  \"AR_x86_64-linux-android\": \"/ndk/llvm-ar\",\n  \
            \"CFLAGS_x86_64-linux-android\": \"--target=x86_64-linux-android26 -O2\",\n  \
            \"QUOTED\": \"a \\\"b\\\" c\\\\d\"\n}\n";
        assert_eq!(
            env_in_json(json),
            Ok(vec![
                ("AR_x86_64-linux-android".into(), "/ndk/llvm-ar".into()),
                (
                    "CFLAGS_x86_64-linux-android".into(),
                    "--target=x86_64-linux-android26 -O2".into()
                ),
                ("QUOTED".into(), "a \"b\" c\\d".into()),
            ])
        );
        assert_eq!(env_in_json("{\n}\n"), Ok(vec![]));
        assert!(env_in_json("{\n  \"KEY\": 1\n}\n").is_err());
        assert!(env_in_json("{\n  \"KEY\": \"unterminated\n}\n").is_err());
    }

    #[test]
    fn the_link_env_adds_cargo_ndks_linker_wrapper_variables_from_its_clang_path() {
        let exported = vec![
            (
                "CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER".to_string(),
                "/bin/cargo-ndk".to_string(),
            ),
            (
                NDK_CLANG_PATH_VARIABLE.to_string(),
                "/ndk/clang".to_string(),
            ),
        ];
        let link = ANDROID_ABIS[3].ndk_link();
        assert_eq!(
            link.link_target,
            format!("--target=x86_64-linux-android{ANDROID_API_LEVEL}")
        );
        let env = ndk_link_env(exported.clone(), &link.link_target).unwrap();
        assert_eq!(&env[..2], &exported[..]);
        assert_eq!(
            &env[2..],
            &[
                (
                    NDK_LINK_CLANG_VARIABLE.to_string(),
                    "/ndk/clang".to_string()
                ),
                (
                    NDK_LINK_TARGET_VARIABLE.to_string(),
                    link.link_target.clone()
                ),
            ]
        );
        assert!(ndk_link_env(vec![], &link.link_target).is_err());
    }

    #[test]
    fn every_abi_links_through_the_clang_target_its_cc_wrapper_names() {
        for abi in &ANDROID_ABIS {
            assert_eq!(abi.cc(), format!("{}-clang", abi.clang_target()));
            assert_eq!(
                abi.ndk_link().link_target,
                format!("--target={}", abi.clang_target())
            );
            assert_eq!(abi.ndk_link().env_command, ndk_env_command(abi.triple));
        }
    }

    #[test]
    fn the_swift_package_names_match_its_manifest() {
        let manifest = include_str!("../../../bindings/swift/Package.swift");
        assert!(manifest.contains(&format!("name: \"{SWIFT_PACKAGE}\"")));
        assert!(manifest.contains(&format!(
            "let builderOutput = \"{SWIFT_PACKAGE_OUTPUT_DIR}\""
        )));
        assert!(manifest.contains(&format!("/{}\"", swift_sources_dir())));
        assert!(std::path::Path::new(SWIFT_PACKAGE_MANIFEST).ends_with("Package.swift"));
    }

    #[test]
    fn the_wallet_crate_builds_no_bindgen() {
        let manifest = include_str!("../../../zingo-ffi/lib/Cargo.toml");
        assert!(!manifest.contains("[[bin]]"));
        assert!(!manifest.contains("\"cli\""));
    }

    #[test]
    fn wallet_args_generate_from_the_built_library_and_place_the_profile_first() {
        let inputs = BindgenInputs {
            language: KOTLIN,
            profile: Profile::Release,
            wallet_workspace: "/w/Cargo.toml",
            target_dir: "/t",
        };
        let args = bindgen_args(&inputs, "/t/release/libzingo.so", "/o");
        assert_eq!(
            args,
            [
                "run",
                "--locked",
                "--target-dir",
                "/t",
                "--release",
                "--manifest-path",
                "/w/Cargo.toml",
                "--package",
                "zingo-uniffi-bindgen",
                "--bin",
                "zingo-uniffi-bindgen",
                "--",
                "generate",
                "--library",
                "/t/release/libzingo.so",
                "--language",
                "kotlin",
                "--out-dir",
                "/o",
            ]
        );
    }

    #[test]
    fn binding_sets_cover_every_generation_in_every_language() {
        assert_eq!(binding_sets().count(), GENERATIONS.len() * LANGUAGES.len());
    }

    #[test]
    fn an_artifact_name_round_trips_and_a_wildcard_segment_is_a_pattern() {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let bundle = artifact_name(BUNDLE_ARTIFACT, "android", commit);
        assert_eq!(bundle, format!("binding-layer-bundle-android-{commit}"));
        assert_eq!(
            artifact_segment(&bundle, BUNDLE_ARTIFACT, commit),
            Some("android")
        );
        let abi = artifact_name(ABI_ARTIFACT, "arm64-v8a", commit);
        assert_eq!(
            artifact_segment(&abi, ABI_ARTIFACT, commit),
            Some("arm64-v8a")
        );
        assert_eq!(artifact_segment(&abi, BUNDLE_ARTIFACT, commit), None);
        assert_eq!(artifact_segment(&bundle, BUNDLE_ARTIFACT, "bbbb"), None);
        assert_eq!(
            artifact_segment(
                &artifact_name(BUNDLE_ARTIFACT, "", commit),
                BUNDLE_ARTIFACT,
                commit
            ),
            None
        );
        assert_eq!(
            artifact_name(BUNDLE_ARTIFACT, ARTIFACT_WILDCARD, commit),
            format!("binding-layer-bundle-*-{commit}")
        );
    }

    #[test]
    fn the_artifact_outputs_name_every_artifact_of_a_commit_once() {
        let commit = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let rendered = render_outputs(&artifact_outputs(&["android", "ios"], commit));
        assert_eq!(
            rendered,
            format!(
                "abis=[\"arm64-v8a\",\"armeabi-v7a\",\"x86\",\"x86_64\"]\n\
                 abi_artifacts={{\"arm64-v8a\":\"binding-layer-abi-arm64-v8a-{commit}\",\
                 \"armeabi-v7a\":\"binding-layer-abi-armeabi-v7a-{commit}\",\
                 \"x86\":\"binding-layer-abi-x86-{commit}\",\
                 \"x86_64\":\"binding-layer-abi-x86_64-{commit}\"}}\n\
                 abi_pattern=binding-layer-abi-*-{commit}\n\
                 aar_glob=*-release.aar\n\
                 bundle_android=binding-layer-bundle-android-{commit}\n\
                 bundle_ios=binding-layer-bundle-ios-{commit}\n\
                 bundle_pattern=binding-layer-bundle-*-{commit}\n\
                 ios_out=bindings/swift/build\n"
            )
        );
    }

    #[test]
    fn android_cc_carries_the_api_level() {
        assert!(ANDROID_ABIS
            .iter()
            .all(|abi| abi.cc().contains(ANDROID_API_LEVEL)));
    }

    #[test]
    fn only_the_proxy_kotlin_generates_where_the_proxy_configuration_applies() {
        assert_eq!(
            bindgen_workdir(Generation::Proxy, KOTLIN),
            Workdir::ProxyCrate
        );
        assert_eq!(
            bindgen_workdir(Generation::Proxy, SWIFT),
            Workdir::WalletWorkspace
        );
        assert!(LANGUAGES
            .iter()
            .all(|language| bindgen_workdir(Generation::Wallet, language) == Workdir::WalletCrate));
    }
}
