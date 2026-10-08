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

pub const BUILD_SCRIPTS_DIR: &str = "build";

pub const ZINGOLIB_BUILD_PREFIX: &str = "zingolib-";

pub const GENERATED_DESCRIPTOR: &str = "out/git_description.txt";

/// The container engines to try, in order.
pub const ENGINES: [&str; 2] = ["podman", "docker"];

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

/// The Swift source directory that the SwiftPM package compiles, relative to the builder's output.
pub const SWIFT_SOURCES_DIR: &str = "Sources/ZingoBindings";

pub const SWIFT_PACKAGE: &str = "ZingoBindings";

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

impl AndroidAbi {
    /// The NDK clang wrapper for this ABI at the builder's API level.
    pub fn cc(&self) -> String {
        format!("{}{ANDROID_API_LEVEL}-clang", self.clang_prefix)
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

/// The first container engine that answers `--version`.
pub fn container_engine() -> Result<&'static str, Vec<String>> {
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
    fn the_descriptor_file_is_the_one_the_build_script_writes() {
        let build_script = include_str!("../../../zingolib/build.rs");
        let file = std::path::Path::new(GENERATED_DESCRIPTOR)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap();
        assert!(build_script.contains(&format!("\"{file}\"")));
    }

    #[test]
    fn the_swift_package_names_match_its_manifest() {
        let manifest = include_str!("../../../bindings/swift/Package.swift");
        assert!(manifest.contains(&format!("name: \"{SWIFT_PACKAGE}\"")));
        assert!(manifest.contains(&format!(
            "let builderOutput = \"{SWIFT_PACKAGE_OUTPUT_DIR}\""
        )));
        assert!(SWIFT_SOURCES_DIR.ends_with(SWIFT_PACKAGE));
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
