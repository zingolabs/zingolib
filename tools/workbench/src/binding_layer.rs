/// The binding languages that zingo-mobile generates.
pub const LANGUAGES: [&str; 2] = ["kotlin", "swift"];

/// The binary in the wallet crate that generates bindings from the UDL file.
const WALLET_BINDGEN_BIN: &str = "uniffi-bindgen";

/// The package that holds the bindgen for library-mode generation.
const LIBRARY_BINDGEN_PACKAGE: &str = "zingo-uniffi-bindgen";

/// The binary that generates the proxy crate's bindings from its built library.
const LIBRARY_BINDGEN_BIN: &str = "zingo-uniffi-bindgen";

/// The wallet crate's library name, from which its library file names derive.
pub const WALLET_LIB_NAME: &str = "zingo";

/// The proxy crate's library name, from which its library file names derive.
pub const PROXY_LIB_NAME: &str = "zingo_nym_proxy_ffi";

/// The proxy crate's package name, which cargo selects in its own workspace.
pub const PROXY_PACKAGE: &str = "zingo-nym-proxy-ffi";

/// The variable that carries zingo-mobile's `git describe` into the wallet's build script.
pub const DESCRIBE_VARIABLE: &str = "ZINGO_MOBILE_GIT_DESCRIBE";

/// The variable that tells cargo where to write build output.
pub const TARGET_DIR_VARIABLE: &str = "CARGO_TARGET_DIR";

/// The container engines to try, in order.
pub const ENGINES: [&str; 2] = ["podman", "docker"];

/// The Android API level that zingo-mobile's builder compiles against.
pub const ANDROID_API_LEVEL: &str = "26";

/// The Android output subdirectory that holds the Kotlin sources the AAR compiles.
pub const KOTLIN_OUT_DIR: &str = "kotlin";

/// The Android output subdirectory that holds the per-ABI libraries the AAR packages.
pub const JNI_LIBS_DIR: &str = "jniLibs";

/// The name under which zingo-mobile ships the wallet's Android library.
pub const ANDROID_WALLET_LIBRARY: &str = "libuniffi_zingo.so";

/// The prefix of a Unix library file name.
pub const LIBRARY_PREFIX: &str = "lib";

/// The suffix of an Android shared library.
pub const SHARED_SUFFIX: &str = ".so";

/// The suffix of an iOS static library.
pub const STATIC_SUFFIX: &str = ".a";

/// The operating system name that Rust reports on macOS.
pub const MACOS: &str = "macos";

/// The wallet XCFramework that zingo-mobile ships.
pub const WALLET_XCFRAMEWORK: &str = "Zingolib.xcframework";

/// The proxy XCFramework that zingo-mobile ships.
pub const PROXY_XCFRAMEWORK: &str = "ZingoNymProxyFFI.xcframework";

/// Both XCFrameworks, in the order that zingo-mobile's builder creates them.
pub const XCFRAMEWORKS: [&str; 2] = [WALLET_XCFRAMEWORK, PROXY_XCFRAMEWORK];

/// The Swift source directory that the SwiftPM package compiles, relative to the builder's output.
pub const SWIFT_SOURCES_DIR: &str = "Sources/ZingoBindings";

/// The wallet's generated Swift source.
pub const WALLET_SWIFT: &str = "zingo.swift";

/// The proxy's generated Swift source.
pub const PROXY_SWIFT: &str = "zingo_nym_proxy_ffi.swift";

/// Both generated Swift sources.
pub const SWIFT_SOURCES: [&str; 2] = [WALLET_SWIFT, PROXY_SWIFT];

/// One Android ABI, with the environment that zingo-mobile's builder sets for it.
pub struct AndroidAbi {
    /// The Rust target triple.
    pub triple: &'static str,
    /// The directory name under `jniLibs`.
    pub jni_dir: &'static str,
    /// The prefix of the NDK clang wrapper, before the API level.
    pub clang_prefix: &'static str,
    /// The value of `CARGO_FEATURE_STD` that zingo-mobile's builder sets.
    pub std_feature: &'static str,
}

impl AndroidAbi {
    /// The NDK clang wrapper for this ABI at zingo-mobile's API level.
    pub fn cc(&self) -> String {
        format!("{}{ANDROID_API_LEVEL}-clang", self.clang_prefix)
    }

    /// The environment that zingo-mobile's builder sets while it builds this ABI.
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

/// Every Android ABI, in the order that zingo-mobile's builder builds them.
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
    /// The wallet crate's bindings, generated from its UDL file.
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
    /// The `--release` profile, which zingo-mobile's builders use.
    Release,
}

impl Profile {
    /// The cargo arguments that select this profile.
    pub const fn cargo_args(self) -> &'static [&'static str] {
        match self {
            Profile::Debug => &[],
            Profile::Release => &["--release"],
        }
    }

    /// The directory under a target directory that this profile writes to.
    pub const fn directory(self) -> &'static str {
        match self {
            Profile::Debug => "debug",
            Profile::Release => "release",
        }
    }
}

/// The paths that a bindgen run reads, as seen by the process that runs it.
pub struct BindgenInputs<'a> {
    /// The wallet crate's manifest.
    pub wallet_crate: &'a str,
    /// The wallet crate's UDL file.
    pub udl: &'a str,
    /// The wallet-side workspace manifest.
    pub wallet_workspace: &'a str,
    /// The built proxy library that library mode reads.
    pub proxy_library: &'a str,
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
    format!("{}-{language}", generation.label())
}

/// The `cargo` arguments that generate one binding set in one language into a directory.
pub fn bindgen_args(
    generation: Generation,
    language: &str,
    inputs: &BindgenInputs,
    out_dir: &str,
    profile: Profile,
) -> Vec<String> {
    let selection: Vec<&str> = match generation {
        Generation::Wallet => vec![
            "--manifest-path",
            inputs.wallet_crate,
            "--bin",
            WALLET_BINDGEN_BIN,
            "--",
            "generate",
            inputs.udl,
        ],
        Generation::Proxy => vec![
            "--manifest-path",
            inputs.wallet_workspace,
            "--package",
            LIBRARY_BINDGEN_PACKAGE,
            "--bin",
            LIBRARY_BINDGEN_BIN,
            "--",
            "generate",
            "--library",
            inputs.proxy_library,
        ],
    };
    [
        &["run", "--locked", "--target-dir", inputs.target_dir],
        profile.cargo_args(),
        selection.as_slice(),
        &["--language", language, "--out-dir", out_dir],
    ]
    .concat()
    .into_iter()
    .map(String::from)
    .collect()
}

/// The file name of a library with the given name, prefix, and suffix.
pub fn library_file(prefix: &str, lib_name: &str, suffix: &str) -> String {
    format!("{prefix}{lib_name}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallet_args_select_the_udl_bindgen_and_place_the_profile_first() {
        let inputs = BindgenInputs {
            wallet_crate: "/w/lib/Cargo.toml",
            udl: "/w/lib/src/zingo.udl",
            wallet_workspace: "/w/Cargo.toml",
            proxy_library: "/p/libzingo_nym_proxy_ffi.so",
            target_dir: "/t",
        };
        let args = bindgen_args(
            Generation::Wallet,
            "kotlin",
            &inputs,
            "/o",
            Profile::Release,
        );
        assert_eq!(
            args,
            [
                "run",
                "--locked",
                "--target-dir",
                "/t",
                "--release",
                "--manifest-path",
                "/w/lib/Cargo.toml",
                "--bin",
                "uniffi-bindgen",
                "--",
                "generate",
                "/w/lib/src/zingo.udl",
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
}
