#![forbid(unsafe_code)]

use std::collections;
use std::env;
use std::fs;
use std::iter;
use std::mem;
use std::path;

use workbench::binding_layer;

/// The program name that prefixes every diagnostic.
const PROGRAM: &str = "binding-copy-gate";

/// The invocation shape, reported when the arguments do not parse.
const USAGE: &str = "usage: binding-copy-gate <import|graph|bindings|artifacts|ios-artifacts|all> \
    --mobile <zingo-mobile checkout at TFC> --tfc <rev> --tar <rev> --import <rev> --placement <rev>";

/// The flag that names the Aligned Rev.
const TAR_FLAG: &str = "--tar";

/// The revision of this checkout that the gates judge.
const HEAD: &str = "HEAD";

/// The flag that names a zingo-mobile checkout at the Freeze Commit.
const MOBILE_FLAG: &str = "--mobile";

/// The flag that names the Freeze Commit.
const TFC_FLAG: &str = "--tfc";

/// The flag that names the import commit.
const IMPORT_FLAG: &str = "--import";

/// The flag that names the placement commit.
const PLACEMENT_FLAG: &str = "--placement";

/// The gate name that selects every gate in order.
const ALL_GATES: &str = "all";

/// The source that cargo prints for a crate fetched from the zingolib repository.
const ZINGOLIB_GIT_SOURCE: &str = "https://github.com/zingolabs/zingolib";

/// The text that opens a source annotation in `cargo tree` output.
const ANNOTATION_OPEN: &str = " (";

/// The character that closes a source annotation in `cargo tree` output.
const ANNOTATION_CLOSE: char = ')';

/// The number that a human gives the first line of a file.
const FIRST_LINE_NUMBER: usize = 1;

/// The number of leading command-line arguments that name the program itself.
const PROGRAM_NAME_ARGUMENTS: usize = 1;

/// The manifests and lockfiles of the copied crates, the only copied paths the branch may change.
const COPIED_MANIFESTS: [&str; 4] = [
    "zingo-ffi/Cargo.toml",
    "zingo-ffi/Cargo.lock",
    "zingo-netutils/nym-proxy-ffi/Cargo.toml",
    "zingo-netutils/nym-proxy-ffi/Cargo.lock",
];

/// The parent manifests whose workspaces the placement excludes the copies from.
const PARENT_MANIFESTS: [&str; 2] = ["Cargo.toml", "zingo-netutils/Cargo.toml"];

/// The directories that the branch adds beside the copy: the packaging and the gate tooling.
const BRANCH_DIRECTORIES: [&str; 2] = ["bindings", "tools/workbench"];

/// The separator between a directory and the paths under it.
const PATH_SEPARATOR: char = '/';

/// The scratch directory, under the zingolib root, that holds generated bindings.
const SCRATCH_DIR: &str = "target/binding-copy-gate";

/// The subdirectory of each side's scratch directory that holds cargo's build output.
const CARGO_TARGET_SUBDIR: &str = "cargo";

/// The cargo profile that the gate builds with, since generated bindings do not depend on it.
const GATE_PROFILE: binding_layer::Profile = binding_layer::Profile::Debug;

/// zingo-mobile's Android Dockerfile at TFC, relative to its root.
const TFC_ANDROID_DOCKERFILE: &str = "rust/android/docker/Dockerfile";

/// The build context that zingo-mobile's Android builder passes, relative to its root.
const TFC_ANDROID_CONTEXT: &str = "rust";

/// The Dockerfile stage that zingo-mobile's Android builder selects.
const TFC_ANDROID_STAGE: &str = "build_android";

/// The tag under which the gate builds zingo-mobile's Android image.
const TFC_ANDROID_IMAGE: &str = "localhost/zingolib/binding-copy-gate-tfc-android";

/// The directory at which zingo-mobile's Android image holds its `rust/` tree.
const TFC_CONTAINER_RUST: &str = "/opt/zingo/rust";

/// The arguments with which zingo-mobile's Android builder describes the checkout.
const DESCRIBE_ARGS: [&str; 6] = [
    "describe", "--dirty", "--always", "--long", "--match", "zingo-*",
];

/// The variable that points podman at a registries configuration.
const REGISTRIES_VARIABLE: &str = "CONTAINERS_REGISTRIES_CONF";

/// The registries configuration that lets podman resolve zingo-mobile's short image name as docker does.
const REGISTRIES_CONF: &str = "unqualified-search-registries = [\"docker.io\"]\n";

/// The file, under the scratch directory, that holds the registries configuration.
const REGISTRIES_FILE: &str = "registries.conf";

/// The scratch subdirectory that holds zingo-mobile's Android artifacts at TFC.
const TFC_ANDROID_DIR: &str = "tfc-android";

/// The scratch subdirectory that holds the unpacked AAR.
const COPY_AAR_DIR: &str = "copy-aar";

/// The AAR that `bundleReleaseAar` produces, relative to the zingolib root.
const COPY_AAR: &str = "bindings/android/build/outputs/aar/zingo-binding-layer-release.aar";

/// The builder output that the AAR packages, relative to the zingolib root.
const COPY_BUILDER_OUTPUT: &str = "bindings/android/build/binding-layer";

/// The directory inside an AAR that holds the per-ABI libraries.
const AAR_JNI_DIR: &str = "jni";

/// The manifest inside an AAR.
const AAR_MANIFEST: &str = "AndroidManifest.xml";

/// The Gradle library's build script, relative to the zingolib root.
const COPY_GRADLE: &str = "bindings/android/build.gradle.kts";

/// zingo-mobile's root Gradle build script at TFC, relative to its root.
const TFC_ROOT_GRADLE: &str = "android/build.gradle.kts";

/// zingo-mobile's app Gradle build script at TFC, relative to its root.
const TFC_APP_GRADLE: &str = "android/app/build.gradle.kts";

/// The text that precedes the minimum SDK in an AAR manifest.
const AAR_MIN_SDK: Declared = Declared {
    marker: "android:minSdkVersion=\"",
    terminator: '"',
};

/// The text that precedes the minimum SDK in zingo-mobile's root build script.
const TFC_MIN_SDK: Declared = Declared {
    marker: "set(\"minSdkVersion\", ",
    terminator: ')',
};

/// The text that precedes the NDK version in zingo-mobile's root build script.
const TFC_NDK: Declared = Declared {
    marker: "set(\"ndkVersion\", \"",
    terminator: '"',
};

/// The text that precedes the NDK version in the Gradle library's build script.
const COPY_NDK: Declared = Declared {
    marker: "bindingNdkVersion = \"",
    terminator: '"',
};

/// The text that precedes the JNA version in either build script.
const JNA: Declared = Declared {
    marker: "net.java.dev.jna:jna:",
    terminator: '"',
};

/// The header of the Cargo release profile.
const RELEASE_PROFILE_HEADER: &str = "[profile.release]";

/// The character that opens every TOML table header.
const TOML_TABLE_OPEN: char = '[';

/// The arguments that list only the names of a library's exported, defined symbols.
const DEFINED_SYMBOL_ARGS: [&str; 3] = ["--defined-only", "--extern-only", "--just-symbol-name"];

/// The argument that restricts a symbol listing to the dynamic symbol table.
const DYNAMIC_SYMBOL_ARG: &str = "--dynamic";

/// The character that ends an archive member's header in a symbol listing.
const ARCHIVE_MEMBER_END: char = ':';

/// The prefix that Mach-O adds to every C symbol name.
const MACHO_PREFIX: char = '_';

/// The prefixes of Rust-mangled symbol names, whose crate hashes depend on the crate's path.
const RUST_MANGLING_PREFIXES: [&str; 2] = ["_ZN", "_R"];

/// The prefix of an LLVM-internal anonymous symbol name, which carries a content hash and a counter.
const LLVM_ANONYMOUS_PREFIX: &str = "anon.";

/// The marker that every LLVM-internal anonymous symbol name contains after its counter.
const LLVM_ANONYMOUS_MARKER: &str = ".llvm.";

/// The bytes that start the zingo-mobile descriptor that the wallet embeds.
const DESCRIPTOR_PREFIX: &[u8] = b"zm_";

/// The punctuation that a descriptor may contain besides letters and digits.
const DESCRIPTOR_PUNCTUATION: &[u8] = b"_.-";

/// The name of the generated function that returns the descriptor, which is not itself a descriptor.
const DESCRIPTOR_FUNCTION: &str = "zm_description";

/// The length of the shortest descriptor body, the abbreviated hash that the wallet's build script keeps.
const DESCRIPTOR_HASH_LENGTH: usize = 5;

/// The length of the shortest descriptor, below which a `zm_` match is only stray bytes.
const DESCRIPTOR_MIN_LENGTH: usize = DESCRIPTOR_PREFIX.len() + DESCRIPTOR_HASH_LENGTH;

/// The tool that lists every library's symbols, which exits zero on archive members without symbols.
const SYMBOL_TOOL: &str = "llvm-nm";

/// How to list and normalize a library's exported symbols.
#[derive(Clone, Copy)]
enum Symbols {
    /// A shared library's dynamic symbols, compared in full.
    Dynamic,
    /// A static library's C-ABI symbols, with Rust-mangled and LLVM-anonymous names left out.
    StaticFfi,
}

impl Symbols {
    /// The tool's arguments before the library path.
    fn args(self) -> Vec<&'static str> {
        match self {
            Symbols::Dynamic => iter::once(DYNAMIC_SYMBOL_ARG)
                .chain(DEFINED_SYMBOL_ARGS)
                .collect(),
            Symbols::StaticFfi => DEFINED_SYMBOL_ARGS.to_vec(),
        }
    }

    /// The listing reduced to what the gate compares.
    fn normalize(self, listing: &str) -> String {
        match self {
            Symbols::Dynamic => listing.to_string(),
            Symbols::StaticFfi => ffi_symbols(listing),
        }
    }
}

/// The manifest at an XCFramework's root, whose library entries `xcodebuild` writes in no fixed order.
const INFO_PLIST: &str = "Info.plist";

/// The key line that opens the XCFramework manifest's list of library entries.
const AVAILABLE_LIBRARIES_KEY: &str = "<key>AvailableLibraries</key>";

/// The line that opens a property-list array.
const PLIST_ARRAY_OPEN: &str = "<array>";

/// The line that closes a property-list array.
const PLIST_ARRAY_CLOSE: &str = "</array>";

/// The line that opens a property-list dictionary.
const PLIST_DICT_OPEN: &str = "<dict>";

/// The line that closes a property-list dictionary.
const PLIST_DICT_CLOSE: &str = "</dict>";

/// The program that runs zingo-mobile's JavaScript builders.
const NODE: &str = "node";

/// zingo-mobile's iOS builder at TFC, relative to its root.
const TFC_IOS_BUILDER: &str = "rust/ios/build_ios.mjs";

/// The directory, relative to zingo-mobile's root, where its iOS builder writes.
const TFC_IOS_DIR: &str = "ios";

/// The SwiftPM package's builder output, relative to the zingolib root.
const COPY_SWIFT_OUTPUT: &str = "bindings/swift/build";

/// The SwiftPM manifest, relative to the zingolib root.
const COPY_PACKAGE: &str = "bindings/swift/Package.swift";

/// The workbench manifest, relative to the zingolib root.
const WORKBENCH_MANIFEST: &str = "tools/workbench/Cargo.toml";

/// The workbench binary that builds the Binding Layer's packaging.
const BUILDER_BIN: &str = "build-binding-layer";

/// The text that precedes the deployment target in zingo-mobile's iOS builder.
const TFC_IOS_TARGET: Declared = Declared {
    marker: "IPHONEOS_DEPLOYMENT_TARGET: '",
    terminator: '\'',
};

/// The text that precedes the deployment target in the SwiftPM manifest.
const COPY_IOS_TARGET: Declared = Declared {
    marker: ".iOS(\"",
    terminator: '"',
};

/// Where a declared value sits in a text: after a marker and before a terminator.
struct Declared {
    /// The text immediately before the value.
    marker: &'static str,
    /// The character immediately after the value.
    terminator: char,
}

impl Declared {
    /// The value in a text, if the marker occurs.
    fn find<'a>(&self, text: &'a str) -> Option<&'a str> {
        text.split_once(self.marker)
            .and_then(|(_, rest)| rest.split_once(self.terminator))
            .map(|(value, _)| value)
    }
}

/// The `cargo tree` arguments that print a crate's complete resolved graph.
const TREE_ARGS: [&str; 11] = [
    "tree",
    "--locked",
    "--all-features",
    "--target",
    "all",
    "--edges",
    "normal,build,dev",
    "--prefix",
    "depth",
    "--format",
    "{p}",
];

/// A path that the import copies, named at TFC and in zingolib.
struct CopiedPath {
    /// The path in zingo-mobile at TFC.
    original: &'static str,
    /// The path in zingolib after the import.
    copy: &'static str,
}

/// Every path that the import copies.
const COPIED_PATHS: [CopiedPath; 5] = [
    CopiedPath {
        original: "rust/lib",
        copy: "zingo-ffi/lib",
    },
    CopiedPath {
        original: "rust/uniffi-bindgen",
        copy: "zingo-ffi/uniffi-bindgen",
    },
    CopiedPath {
        original: "rust/nym-proxy-ffi",
        copy: "zingo-netutils/nym-proxy-ffi",
    },
    CopiedPath {
        original: "rust/Cargo.toml",
        copy: "zingo-ffi/Cargo.toml",
    },
    CopiedPath {
        original: "rust/Cargo.lock",
        copy: "zingo-ffi/Cargo.lock",
    },
];

/// The workspace that a copied crate resolves in.
#[derive(Clone, Copy)]
enum Workspace {
    /// The wallet-side workspace of the wallet crate and the bindgen.
    Wallet,
    /// The proxy crate's own workspace.
    Proxy,
}

/// Where one side keeps the files that the gates read, relative to its root.
struct Layout {
    /// The wallet-side workspace manifest.
    wallet_workspace: &'static str,
    /// The wallet crate's manifest.
    wallet_crate: &'static str,
    /// The wallet crate's UDL file.
    udl: &'static str,
    /// The proxy crate's workspace manifest.
    proxy_workspace: &'static str,
}

impl Layout {
    /// The manifest of the given workspace in this layout.
    fn manifest(&self, workspace: Workspace) -> &'static str {
        match workspace {
            Workspace::Wallet => self.wallet_workspace,
            Workspace::Proxy => self.proxy_workspace,
        }
    }
}

/// zingo-mobile's layout at TFC.
const TFC_LAYOUT: Layout = Layout {
    wallet_workspace: "rust/Cargo.toml",
    wallet_crate: "rust/lib/Cargo.toml",
    udl: "rust/lib/src/zingo.udl",
    proxy_workspace: "rust/nym-proxy-ffi/Cargo.toml",
};

/// zingolib's layout after the placement.
const COPY_LAYOUT: Layout = Layout {
    wallet_workspace: "zingo-ffi/Cargo.toml",
    wallet_crate: "zingo-ffi/lib/Cargo.toml",
    udl: "zingo-ffi/lib/src/zingo.udl",
    proxy_workspace: "zingo-netutils/nym-proxy-ffi/Cargo.toml",
};

/// A copied crate whose resolved graph gate 2 compares.
struct GatedCrate {
    /// The package name that cargo selects.
    package: &'static str,
    /// The workspace that the package resolves in.
    workspace: Workspace,
}

/// Every copied crate that gate 2 compares.
const GATED_CRATES: [GatedCrate; 3] = [
    GatedCrate {
        package: "zingo",
        workspace: Workspace::Wallet,
    },
    GatedCrate {
        package: "zingo-uniffi-bindgen",
        workspace: Workspace::Wallet,
    },
    GatedCrate {
        package: "zingo-nym-proxy-ffi",
        workspace: Workspace::Proxy,
    },
];

/// One equivalence gate.
#[derive(Clone, Copy)]
enum Gate {
    /// Gate 1, which compares the imported object hashes with TFC's.
    Import,
    /// Gate 2, which checks the placement and compares resolved graphs.
    Graph,
    /// Gate 3's first half, which compares the generated bindings.
    Bindings,
    /// Gate 3's Android half, which compares zingo-mobile's Android artifacts with the AAR.
    Artifacts,
    /// Gate 3's iOS half, which compares zingo-mobile's XCFrameworks with the SwiftPM package's.
    IosArtifacts,
}

/// Every gate, in the order that the remedies assume.
const GATES: [Gate; 5] = [
    Gate::Import,
    Gate::Graph,
    Gate::Bindings,
    Gate::Artifacts,
    Gate::IosArtifacts,
];

impl Gate {
    /// The gate that a command-line name selects.
    fn named(name: &str) -> Option<Gate> {
        GATES.into_iter().find(|gate| gate.name() == name)
    }

    /// The command-line name of this gate.
    fn name(self) -> &'static str {
        match self {
            Gate::Import => "import",
            Gate::Graph => "graph",
            Gate::Bindings => "bindings",
            Gate::Artifacts => "artifacts",
            Gate::IosArtifacts => "ios-artifacts",
        }
    }

    /// What this host lacks to run this gate, if anything.
    fn unmet_requirement(self) -> Option<&'static str> {
        match self {
            Gate::IosArtifacts if env::consts::OS != binding_layer::MACOS => {
                Some("needs macOS with Xcode")
            }
            _ => None,
        }
    }
}

/// The parsed command line.
struct Invocation<'a> {
    /// The gates to run, in order.
    gates: Vec<Gate>,
    /// Whether the selection was `all`, which reports gates this host cannot run as skipped.
    all: bool,
    /// The zingo-mobile checkout at TFC.
    mobile: path::PathBuf,
    /// The Freeze Commit.
    tfc: &'a str,
    /// The Aligned Rev.
    tar: &'a str,
    /// The import commit.
    import: &'a str,
    /// The placement commit.
    placement: &'a str,
}

/// One checkout that the gates read, either zingo-mobile at TFC or zingolib.
struct Side {
    /// The side's name, which also names its scratch directory.
    name: &'static str,
    /// The checkout's root directory.
    root: path::PathBuf,
    /// Where the checkout keeps the gated files.
    layout: Layout,
}

impl Side {
    /// The absolute path of a file that this side's layout names.
    fn file(&self, relative: &str) -> path::PathBuf {
        self.root.join(relative)
    }

    /// The absolute directory that a bindgen working directory names on this side.
    fn workdir(&self, workdir: binding_layer::Workdir) -> path::PathBuf {
        let manifest = match workdir {
            binding_layer::Workdir::WalletCrate => self.layout.wallet_crate,
            binding_layer::Workdir::WalletWorkspace => self.layout.manifest(Workspace::Wallet),
            binding_layer::Workdir::ProxyCrate => self.layout.manifest(Workspace::Proxy),
        };
        self.file(manifest)
            .parent()
            .map_or_else(|| self.root.clone(), path::Path::to_path_buf)
    }
}

/// The outcome of one comparison inside a gate.
struct Check {
    /// What the comparison covered.
    label: String,
    /// Nothing on a match, or the first difference found.
    outcome: Result<(), String>,
}

fn main() {
    let args: Vec<String> = env::args().skip(PROGRAM_NAME_ARGUMENTS).collect();
    workbench::run(
        PROGRAM,
        || gate_all(&args),
        |report| report.iter().for_each(|line| println!("{line}")),
    )
}

/// Run the selected gates and return their report, or the diagnostics of the first failing gate.
fn gate_all(args: &[String]) -> Result<Vec<String>, Vec<String>> {
    let invocation = parse(args)?;
    let tfc_side = Side {
        name: "tfc",
        root: resolved(&invocation.mobile)?,
        layout: TFC_LAYOUT,
    };
    let copy_side = Side {
        name: "copy",
        root: resolved(&workbench::repo_root()?)?,
        layout: COPY_LAYOUT,
    };
    require_checkout_at(&tfc_side.root, invocation.tfc)?;
    invocation
        .gates
        .iter()
        .map(|gate| match gate.unmet_requirement() {
            Some(requirement) if invocation.all => {
                Ok(vec![format!("gate {} SKIPPED  {requirement}", gate.name())])
            }
            _ => run_gate(*gate, &invocation, &tfc_side, &copy_side),
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|reports| reports.concat())
}

/// A checkout's real path, since `cargo tree` prints real paths even when a checkout is reached through a symbolic link.
fn resolved(checkout: &path::Path) -> Result<path::PathBuf, Vec<String>> {
    fs::canonicalize(checkout)
        .map_err(|e| vec![format!("cannot resolve {}: {e}", checkout.display())])
}

/// Parse the gate selection and its flags, all of which are required.
fn parse(args: &[String]) -> Result<Invocation<'_>, Vec<String>> {
    let (selection, flags) = args.split_first().ok_or_else(|| vec![USAGE.to_string()])?;
    let all = selection == ALL_GATES;
    let gates = if all {
        GATES.to_vec()
    } else {
        vec![Gate::named(selection)
            .ok_or_else(|| vec![format!("unknown gate `{selection}`"), USAGE.to_string()])?]
    };
    let required = |flag: &str| {
        workbench::flag_value(flags, flag)?
            .ok_or_else(|| vec![format!("missing {flag}"), USAGE.to_string()])
    };
    Ok(Invocation {
        gates,
        all,
        mobile: path::PathBuf::from(required(MOBILE_FLAG)?),
        tfc: required(TFC_FLAG)?,
        tar: required(TAR_FLAG)?,
        import: required(IMPORT_FLAG)?,
        placement: required(PLACEMENT_FLAG)?,
    })
}

/// Fail unless the zingo-mobile checkout's HEAD is the given Freeze Commit.
fn require_checkout_at(checkout: &path::Path, tfc: &str) -> Result<(), Vec<String>> {
    let head = commit_id(checkout, "HEAD")?;
    let expected = commit_id(checkout, tfc)?;
    if head == expected {
        Ok(())
    } else {
        Err(vec![format!(
            "{} is at {head}, not at TFC {expected}",
            checkout.display()
        )])
    }
}

/// Run one gate and return its report, or its diagnostics.
fn run_gate(
    gate: Gate,
    invocation: &Invocation,
    tfc_side: &Side,
    copy_side: &Side,
) -> Result<Vec<String>, Vec<String>> {
    let checks = match gate {
        Gate::Import => import_checks(invocation, tfc_side, copy_side)?,
        Gate::Graph => graph_checks(invocation, tfc_side, copy_side)?,
        Gate::Bindings => binding_checks(tfc_side, copy_side)?,
        Gate::Artifacts => artifact_checks(tfc_side, copy_side)?,
        Gate::IosArtifacts => ios_artifact_checks(tfc_side, copy_side)?,
    };
    verdict(gate, &checks)
}

/// Gate 1: every copied path has the same object hash at TFC and at the import commit.
fn import_checks(
    invocation: &Invocation,
    tfc_side: &Side,
    copy_side: &Side,
) -> Result<Vec<Check>, Vec<String>> {
    COPIED_PATHS
        .iter()
        .map(|copied| {
            let original = object_id(&tfc_side.root, invocation.tfc, copied.original)?;
            let copy = object_id(&copy_side.root, invocation.import, copied.copy)?;
            Ok(Check {
                label: format!("{} -> {}", copied.original, copied.copy),
                outcome: equal_or_describe(&original, &copy),
            })
        })
        .collect()
}

/// Gate 2: the branch changes only what it may, starts at TAR, and each crate's graph matches TFC's.
fn graph_checks(
    invocation: &Invocation,
    tfc_side: &Side,
    copy_side: &Side,
) -> Result<Vec<Check>, Vec<String>> {
    let copy_root = workbench::utf8(&copy_side.root)?;
    let tar = commit_id(&copy_side.root, invocation.tar)?;
    let changed = |from: &str, to: &str, pathspecs: &[&str]| {
        workbench::git(
            &[
                ["-C", copy_root, "diff", "--name-only", from, to, "--"].as_slice(),
                pathspecs,
            ]
            .concat(),
        )
    };
    let copied: Vec<&str> = COPIED_PATHS.iter().map(|copied| copied.copy).collect();
    let placement_check = paths_check(
        "placement touches only its intended manifests",
        &changed(invocation.import, invocation.placement, &[])?,
        is_placement_path,
    );
    let copied_check = paths_check(
        "copied paths change after the import only in their manifests",
        &changed(invocation.import, HEAD, &copied)?,
        |touched| COPIED_MANIFESTS.contains(&touched),
    );
    let ancestry_check = Check {
        label: format!("the branch descends from TAR {}", invocation.tar),
        outcome: workbench::git(&["-C", copy_root, "merge-base", "--is-ancestor", &tar, HEAD])
            .map(drop)
            .map_err(|_| "TAR is not an ancestor of HEAD".to_string()),
    };
    let tar_check = paths_check(
        "zingolib outside the copy, the placement, and the packaging is TAR's",
        &changed(&tar, HEAD, &[])?,
        |touched| is_branch_path(touched, &copied),
    );
    let source_prefixes = [
        ZINGOLIB_GIT_SOURCE,
        workbench::utf8(&tfc_side.root)?,
        workbench::utf8(&copy_side.root)?,
    ];
    let tree_checks = GATED_CRATES
        .iter()
        .map(|gated| {
            let tfc_tree = resolved_tree(tfc_side, gated, &source_prefixes)?;
            let copy_tree = resolved_tree(copy_side, gated, &source_prefixes)?;
            Ok(Check {
                label: format!("resolved graph of {}", gated.package),
                outcome: first_difference(&tfc_tree, &copy_tree),
            })
        })
        .collect::<Result<Vec<_>, Vec<String>>>()?;
    Ok([placement_check, copied_check, ancestry_check, tar_check]
        .into_iter()
        .chain(tree_checks)
        .collect())
}

/// Gate 3: the bindings generated on each side match byte for byte.
fn binding_checks(tfc_side: &Side, copy_side: &Side) -> Result<Vec<Check>, Vec<String>> {
    let scratch = copy_side.file(SCRATCH_DIR);
    generate_bindings(tfc_side, &scratch)?;
    generate_bindings(copy_side, &scratch)?;
    binding_layer::binding_sets()
        .map(|(generation, language)| {
            let output = binding_layer::output_name(generation, language);
            let tfc_files = file_map(&scratch.join(tfc_side.name).join(&output))?;
            let copy_files = file_map(&scratch.join(copy_side.name).join(&output))?;
            Ok(Check {
                label: format!("{output} bindings"),
                outcome: first_file_difference(&tfc_files, &copy_files),
            })
        })
        .collect()
}

/// Generate every binding set for one side into its scratch directory.
fn generate_bindings(side: &Side, scratch: &path::Path) -> Result<(), Vec<String>> {
    let side_dir = scratch.join(side.name);
    let target_dir = side_dir.join(CARGO_TARGET_SUBDIR);
    let proxy_library = build_proxy_library(side, &target_dir)?;
    let wallet_crate = side.file(side.layout.wallet_crate);
    let udl = side.file(side.layout.udl);
    let wallet_workspace = side.file(side.layout.manifest(Workspace::Wallet));
    let inputs = binding_layer::BindgenInputs {
        wallet_crate: workbench::utf8(&wallet_crate)?,
        udl: workbench::utf8(&udl)?,
        wallet_workspace: workbench::utf8(&wallet_workspace)?,
        proxy_library: workbench::utf8(&proxy_library)?,
        target_dir: workbench::utf8(&target_dir)?,
    };
    binding_layer::binding_sets().try_for_each(|(generation, language)| {
        let out_dir =
            workbench::fresh_dir(&side_dir.join(binding_layer::output_name(generation, language)))?;
        let args = binding_layer::bindgen_args(
            generation,
            language,
            &inputs,
            workbench::utf8(&out_dir)?,
            GATE_PROFILE,
        );
        workbench::stdout_in(
            &side.workdir(binding_layer::bindgen_workdir(generation, language)),
            "cargo",
            &args.iter().map(String::as_str).collect::<Vec<_>>(),
            &[],
        )
        .map(drop)
    })
}

/// Build the proxy crate's dynamic library for the host and return its path.
fn build_proxy_library(side: &Side, target_dir: &path::Path) -> Result<path::PathBuf, Vec<String>> {
    workbench::stdout_of(
        "cargo",
        &[
            [
                "build",
                "--locked",
                "--manifest-path",
                workbench::utf8(&side.file(side.layout.manifest(Workspace::Proxy)))?,
                "--target-dir",
                workbench::utf8(target_dir)?,
                "--lib",
            ]
            .as_slice(),
            GATE_PROFILE.cargo_args(),
        ]
        .concat(),
    )?;
    Ok(target_dir
        .join(GATE_PROFILE.directory())
        .join(binding_layer::library_file(
            env::consts::DLL_PREFIX,
            binding_layer::PROXY_LIB_NAME,
            env::consts::DLL_SUFFIX,
        )))
}

/// Print one crate's complete resolved graph on one side, with the named sources erased.
fn resolved_tree(
    side: &Side,
    gated: &GatedCrate,
    source_prefixes: &[&str],
) -> Result<String, Vec<String>> {
    let manifest = side.file(side.layout.manifest(gated.workspace));
    let location = [
        "--manifest-path",
        workbench::utf8(&manifest)?,
        "--package",
        gated.package,
    ];
    workbench::stdout_of("cargo", &[TREE_ARGS.as_slice(), &location].concat())
        .map(|tree| erase_sources(&tree, source_prefixes))
}

/// The full object id of `<rev>:<file>` in a repository.
fn object_id(repository: &path::Path, rev: &str, file: &str) -> Result<String, Vec<String>> {
    workbench::git(&[
        "-C",
        workbench::utf8(repository)?,
        "rev-parse",
        &format!("{rev}:{file}"),
    ])
    .map(|id| id.trim().to_string())
}

/// The full commit id that a rev names in a repository.
fn commit_id(repository: &path::Path, rev: &str) -> Result<String, Vec<String>> {
    workbench::git(&[
        "-C",
        workbench::utf8(repository)?,
        "rev-parse",
        "--verify",
        &format!("{rev}^{{commit}}"),
    ])
    .map(|id| id.trim().to_string())
}

/// Every file under a directory, keyed by its path relative to that directory.
fn file_map(
    directory: &path::Path,
) -> Result<collections::BTreeMap<path::PathBuf, Vec<u8>>, Vec<String>> {
    files_under(directory)?
        .into_iter()
        .map(|file| {
            let contents = fs::read(&file)
                .map_err(|e| vec![format!("cannot read {}: {e}", file.display())])?;
            let relative = file
                .strip_prefix(directory)
                .map_err(|e| vec![format!("{} escapes its directory: {e}", file.display())])?;
            Ok((relative.to_path_buf(), contents))
        })
        .collect()
}

/// Every regular file under a directory, found recursively.
fn files_under(directory: &path::Path) -> Result<Vec<path::PathBuf>, Vec<String>> {
    fs::read_dir(directory)
        .map_err(|e| vec![format!("cannot list {}: {e}", directory.display())])?
        .map(|entry| {
            let entry_path = entry
                .map_err(|e| vec![format!("cannot list {}: {e}", directory.display())])?
                .path();
            if entry_path.is_dir() {
                files_under(&entry_path)
            } else {
                Ok(vec![entry_path])
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|nested| nested.concat())
}

/// Gate 3's Android half: zingo-mobile's artifacts at TFC against the AAR and its builder output.
fn artifact_checks(tfc_side: &Side, copy_side: &Side) -> Result<Vec<Check>, Vec<String>> {
    let scratch = copy_side.file(SCRATCH_DIR);
    let tfc_dir = workbench::fresh_dir(&scratch.join(TFC_ANDROID_DIR))?;
    build_tfc_android(&tfc_side.root, &scratch, &tfc_dir)?;
    let aar_dir = unpack_aar(&copy_side.file(COPY_AAR), &scratch.join(COPY_AAR_DIR))?;
    let tfc_jni = tfc_dir.join(binding_layer::JNI_LIBS_DIR);
    let copy_jni = aar_dir.join(AAR_JNI_DIR);
    let tfc_abis = dir_names(&tfc_jni)?;
    let abi_check = Check {
        label: "Android ABI set".to_string(),
        outcome: set_difference(&tfc_abis, &dir_names(&copy_jni)?),
    };
    let library_checks = tfc_abis
        .iter()
        .map(|abi| {
            let tfc_files = dir_names(&tfc_jni.join(abi))?;
            let copy_files = dir_names(&copy_jni.join(abi))?;
            let symbols = tfc_files
                .intersection(&copy_files)
                .map(|library| {
                    let relative = path::Path::new(abi).join(library);
                    library_pair_checks(
                        &relative.display().to_string(),
                        &tfc_jni.join(&relative),
                        &copy_jni.join(&relative),
                        Symbols::Dynamic,
                    )
                })
                .collect::<Result<Vec<Vec<Check>>, Vec<String>>>()?
                .into_iter()
                .flatten();
            Ok(iter::once(Check {
                label: format!("libraries for {abi}"),
                outcome: set_difference(&tfc_files, &copy_files),
            })
            .chain(symbols)
            .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<Vec<Check>>, Vec<String>>>()?
        .into_iter()
        .flatten();
    let kotlin_check = Check {
        label: "generated Kotlin sources".to_string(),
        outcome: first_file_difference(
            &file_map(&tfc_dir.join(binding_layer::KOTLIN_OUT_DIR))?,
            &file_map(
                &copy_side
                    .file(COPY_BUILDER_OUTPUT)
                    .join(binding_layer::KOTLIN_OUT_DIR),
            )?,
        ),
    };
    let tfc_root_gradle = workbench::read(&tfc_side.file(TFC_ROOT_GRADLE))?;
    let tfc_app_gradle = workbench::read(&tfc_side.file(TFC_APP_GRADLE))?;
    let copy_gradle = workbench::read(&copy_side.file(COPY_GRADLE))?;
    let aar_manifest = workbench::read(&aar_dir.join(AAR_MANIFEST))?;
    let declared_checks = [
        declared_check(
            "minimum SDK",
            TFC_MIN_SDK.find(&tfc_root_gradle),
            AAR_MIN_SDK.find(&aar_manifest),
        ),
        declared_check(
            "NDK version",
            TFC_NDK.find(&tfc_root_gradle),
            COPY_NDK.find(&copy_gradle),
        ),
        declared_check(
            "JNA version",
            JNA.find(&tfc_app_gradle),
            JNA.find(&copy_gradle),
        ),
        release_profile_check(tfc_side, copy_side)?,
    ];
    Ok(iter::once(abi_check)
        .chain(library_checks)
        .chain(iter::once(kotlin_check))
        .chain(declared_checks)
        .collect())
}

/// Build zingo-mobile's Android image at TFC as its builder does, and copy its artifacts out.
fn build_tfc_android(
    tfc_root: &path::Path,
    scratch: &path::Path,
    tfc_dir: &path::Path,
) -> Result<(), Vec<String>> {
    let engine = binding_layer::container_engine()?;
    let registries = scratch.join(REGISTRIES_FILE);
    fs::write(&registries, REGISTRIES_CONF)
        .map_err(|e| vec![format!("cannot write {}: {e}", registries.display())])?;
    let env = [(REGISTRIES_VARIABLE, workbench::utf8(&registries)?)];
    let describe_arg = format!(
        "{}={}",
        binding_layer::DESCRIBE_VARIABLE,
        tfc_describe(tfc_root)?
    );
    workbench::stdout_with_env(
        engine,
        &[
            "build",
            "--target",
            TFC_ANDROID_STAGE,
            "--build-arg",
            &describe_arg,
            "--tag",
            TFC_ANDROID_IMAGE,
            "--file",
            workbench::utf8(&tfc_root.join(TFC_ANDROID_DOCKERFILE))?,
            workbench::utf8(&tfc_root.join(TFC_ANDROID_CONTEXT))?,
        ],
        &env,
    )?;
    let created = workbench::stdout_with_env(engine, &["create", TFC_ANDROID_IMAGE], &env)?;
    let id = created.trim();
    let copied = tfc_android_files().iter().try_for_each(|(from, to)| {
        let destination = tfc_dir.join(to);
        workbench::create_parent(&destination)?;
        workbench::stdout_with_env(
            engine,
            &[
                "cp",
                &format!("{id}:{from}"),
                workbench::utf8(&destination)?,
            ],
            &env,
        )
        .map(drop)
    });
    workbench::stdout_with_env(engine, &["rm", "--volumes", id], &env)?;
    copied
}

/// Every artifact that zingo-mobile's Android builder copies out of its image, and where it lands.
fn tfc_android_files() -> Vec<(String, String)> {
    let release = binding_layer::Profile::Release.directory();
    let shared = |lib_name| {
        binding_layer::library_file(
            binding_layer::LIBRARY_PREFIX,
            lib_name,
            binding_layer::SHARED_SUFFIX,
        )
    };
    let wallet = shared(binding_layer::WALLET_LIB_NAME);
    let proxy = shared(binding_layer::PROXY_LIB_NAME);
    let jni_root = binding_layer::JNI_LIBS_DIR;
    let wallet_name = binding_layer::ANDROID_WALLET_LIBRARY;
    let libraries = binding_layer::ANDROID_ABIS.iter().flat_map(|abi| {
        let triple = abi.triple;
        let jni = format!("{jni_root}/{}", abi.jni_dir);
        [
            (
                format!("{TFC_CONTAINER_RUST}/target/{triple}/{release}/{wallet}"),
                format!("{jni}/{wallet_name}"),
            ),
            (
                format!("{TFC_CONTAINER_RUST}/nym-proxy-ffi/target/{triple}/{release}/{proxy}"),
                format!("{jni}/{proxy}"),
            ),
        ]
    });
    let kotlin_root = binding_layer::KOTLIN_OUT_DIR;
    let wallet_kotlin = kotlin_file(binding_layer::WALLET_LIB_NAME);
    let proxy_kotlin = kotlin_file(binding_layer::PROXY_LIB_NAME);
    let sources = [
        (
            format!("{TFC_CONTAINER_RUST}/lib/src/{wallet_kotlin}"),
            format!("{kotlin_root}/{wallet_kotlin}"),
        ),
        (
            format!("{TFC_CONTAINER_RUST}/nym-proxy-ffi/generated-kotlin/{proxy_kotlin}"),
            format!("{kotlin_root}/{proxy_kotlin}"),
        ),
    ];
    libraries.chain(sources).collect()
}

/// The path, under a Kotlin source root, of the bindings that UniFFI generates for a library.
fn kotlin_file(lib_name: &str) -> String {
    format!("uniffi/{lib_name}/{lib_name}.kt")
}

/// Unpack an AAR into a fresh directory and return that directory.
fn unpack_aar(aar: &path::Path, directory: &path::Path) -> Result<path::PathBuf, Vec<String>> {
    if !aar.is_file() {
        return Err(vec![format!(
            "no AAR at {}: run the Gradle library's bundleReleaseAar first",
            aar.display()
        )]);
    }
    let unpacked = workbench::fresh_dir(directory)?;
    workbench::stdout_of(
        "unzip",
        &[
            "-q",
            "-o",
            workbench::utf8(aar)?,
            "-d",
            workbench::utf8(&unpacked)?,
        ],
    )?;
    Ok(unpacked)
}

/// The names of the entries directly under a directory.
fn dir_names(directory: &path::Path) -> Result<collections::BTreeSet<String>, Vec<String>> {
    fs::read_dir(directory)
        .map_err(|e| vec![format!("cannot list {}: {e}", directory.display())])?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .map_err(|e| vec![format!("cannot list {}: {e}", directory.display())])
        })
        .collect()
}

/// A library's exported, defined symbols, listed and normalized the given way.
fn exported_symbols(library: &path::Path, symbols: Symbols) -> Result<String, Vec<String>> {
    workbench::stdout_of(
        SYMBOL_TOOL,
        &[symbols.args().as_slice(), &[workbench::utf8(library)?]].concat(),
    )
    .map(|listing| symbols.normalize(&listing))
}

/// The symbol check of one library that both sides carry, and its descriptor check if it is the wallet's.
fn library_pair_checks(
    label: &str,
    tfc_library: &path::Path,
    copy_library: &path::Path,
    symbols: Symbols,
) -> Result<Vec<Check>, Vec<String>> {
    let descriptors = |library: &path::Path| {
        fs::read(library)
            .map(|bytes| embedded_descriptors(&bytes))
            .map_err(|e| vec![format!("cannot read {}: {e}", library.display())])
    };
    let symbol_check = Check {
        label: format!("exported symbols of {label}"),
        outcome: first_difference(
            &exported_symbols(tfc_library, symbols)?,
            &exported_symbols(copy_library, symbols)?,
        ),
    };
    let descriptor_check = is_wallet_library(tfc_library)
        .then(|| -> Result<Check, Vec<String>> {
            Ok(Check {
                label: format!("embedded zingo-mobile descriptor of {label}"),
                outcome: set_difference(&descriptors(tfc_library)?, &descriptors(copy_library)?),
            })
        })
        .transpose()?;
    Ok(iter::once(symbol_check).chain(descriptor_check).collect())
}

/// Whether a library file is the wallet's, the only library that embeds zingo-mobile's descriptor.
fn is_wallet_library(library: &path::Path) -> bool {
    let wallet_static = binding_layer::library_file(
        binding_layer::LIBRARY_PREFIX,
        binding_layer::WALLET_LIB_NAME,
        binding_layer::STATIC_SUFFIX,
    );
    library
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == binding_layer::ANDROID_WALLET_LIBRARY || name == wallet_static)
}

/// The C-ABI symbol names in a listing, sorted and without Rust-mangled names or archive headers.
fn ffi_symbols(listing: &str) -> String {
    listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.ends_with(ARCHIVE_MEMBER_END))
        .map(|name| name.strip_prefix(MACHO_PREFIX).unwrap_or(name))
        .filter(|name| {
            !RUST_MANGLING_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix))
        })
        .filter(|name| {
            !(name.starts_with(LLVM_ANONYMOUS_PREFIX) && name.contains(LLVM_ANONYMOUS_MARKER))
        })
        .collect::<collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every zingo-mobile descriptor that a library's bytes contain.
fn embedded_descriptors(bytes: &[u8]) -> collections::BTreeSet<String> {
    bytes
        .windows(DESCRIPTOR_PREFIX.len())
        .enumerate()
        .filter(|(_, window)| *window == DESCRIPTOR_PREFIX)
        .filter_map(|(start, _)| bytes.get(start..))
        .map(|tail| {
            tail.iter()
                .take_while(|byte| {
                    byte.is_ascii_alphanumeric() || DESCRIPTOR_PUNCTUATION.contains(byte)
                })
                .map(|byte| char::from(*byte))
                .collect::<String>()
        })
        .filter(|candidate| {
            candidate.len() >= DESCRIPTOR_MIN_LENGTH && !candidate.starts_with(DESCRIPTOR_FUNCTION)
        })
        .collect()
}

/// The descriptor that zingo-mobile's builders compute for a checkout.
fn tfc_describe(tfc_root: &path::Path) -> Result<String, Vec<String>> {
    workbench::git(
        &[
            ["-C", workbench::utf8(tfc_root)?].as_slice(),
            DESCRIBE_ARGS.as_slice(),
        ]
        .concat(),
    )
    .map(|describe| describe.trim().to_string())
}

/// A check that both sides declare the same Cargo release profile.
fn release_profile_check(tfc_side: &Side, copy_side: &Side) -> Result<Check, Vec<String>> {
    let tfc_cargo = workbench::read(&tfc_side.file(TFC_LAYOUT.wallet_workspace))?;
    let copy_cargo = workbench::read(&copy_side.file(COPY_LAYOUT.wallet_workspace))?;
    Ok(declared_check(
        "Cargo release profile",
        toml_section(&tfc_cargo, RELEASE_PROFILE_HEADER).as_deref(),
        toml_section(&copy_cargo, RELEASE_PROFILE_HEADER).as_deref(),
    ))
}

/// Gate 3's iOS half: zingo-mobile's XCFrameworks and Swift at TFC against the SwiftPM package's.
fn ios_artifact_checks(tfc_side: &Side, copy_side: &Side) -> Result<Vec<Check>, Vec<String>> {
    if let Some(requirement) = Gate::IosArtifacts.unmet_requirement() {
        return Err(vec![format!("the iOS half of gate 3 {requirement}")]);
    }
    let describe = tfc_describe(&tfc_side.root)?;
    let describe_entry = (binding_layer::DESCRIBE_VARIABLE, describe.as_str());
    let env = [describe_entry];
    let tfc_env = [
        describe_entry,
        (
            binding_layer::TOOLCHAIN_VARIABLE,
            binding_layer::BUILDER_TOOLCHAIN,
        ),
    ];
    workbench::run_streaming(
        NODE,
        &[workbench::utf8(&tfc_side.file(TFC_IOS_BUILDER))?],
        &tfc_env,
    )?;
    let copy_out = copy_side.file(COPY_SWIFT_OUTPUT);
    workbench::run_streaming(
        "cargo",
        &[
            "run",
            "--quiet",
            "--manifest-path",
            workbench::utf8(&copy_side.file(WORKBENCH_MANIFEST))?,
            "--bin",
            BUILDER_BIN,
            "--",
            "ios",
            "--out",
            workbench::utf8(&copy_out)?,
        ],
        &env,
    )?;
    let tfc_ios = tfc_side.file(TFC_IOS_DIR);
    let xcframework_checks = binding_layer::XCFRAMEWORKS
        .iter()
        .map(|name| xcframework_checks(name, &tfc_ios.join(name), &copy_out.join(name)))
        .collect::<Result<Vec<Vec<Check>>, Vec<String>>>()?
        .into_iter()
        .flatten();
    let named_sources = |directory: &path::Path| {
        binding_layer::SWIFT_SOURCES
            .iter()
            .map(|source| {
                let file = directory.join(source);
                fs::read(&file)
                    .map(|bytes| (path::PathBuf::from(source), bytes))
                    .map_err(|e| vec![format!("cannot read {}: {e}", file.display())])
            })
            .collect::<Result<collections::BTreeMap<_, _>, Vec<String>>>()
    };
    let swift_check = Check {
        label: "generated Swift sources".to_string(),
        outcome: first_file_difference(
            &named_sources(&tfc_ios)?,
            &named_sources(&copy_out.join(binding_layer::SWIFT_SOURCES_DIR))?,
        ),
    };
    let deployment_check = declared_check(
        "iOS deployment target",
        TFC_IOS_TARGET.find(&workbench::read(&tfc_side.file(TFC_IOS_BUILDER))?),
        COPY_IOS_TARGET.find(&workbench::read(&copy_side.file(COPY_PACKAGE))?),
    );
    Ok(xcframework_checks
        .chain([
            swift_check,
            deployment_check,
            release_profile_check(tfc_side, copy_side)?,
        ])
        .collect())
}

/// The checks of one XCFramework: its file list, its static libraries, and every other file's bytes.
fn xcframework_checks(
    name: &str,
    tfc_dir: &path::Path,
    copy_dir: &path::Path,
) -> Result<Vec<Check>, Vec<String>> {
    let tfc_files = relative_files(tfc_dir)?;
    let copy_files = relative_files(copy_dir)?;
    let listing = Check {
        label: format!("files of {name}"),
        outcome: set_difference(&tfc_files, &copy_files),
    };
    let per_file = tfc_files
        .intersection(&copy_files)
        .map(|relative| {
            let label = format!("{name}/{relative}");
            let tfc_file = tfc_dir.join(relative);
            let copy_file = copy_dir.join(relative);
            if relative.ends_with(binding_layer::STATIC_SUFFIX) {
                library_pair_checks(&label, &tfc_file, &copy_file, Symbols::StaticFfi)
            } else if relative == INFO_PLIST {
                Ok(vec![Check {
                    label: format!("contents of {label}"),
                    outcome: equal_xcframework_plists(&tfc_file, &copy_file)?,
                }])
            } else {
                Ok(vec![Check {
                    label: format!("bytes of {label}"),
                    outcome: equal_bytes(&tfc_file, &copy_file)?,
                }])
            }
        })
        .collect::<Result<Vec<Vec<Check>>, Vec<String>>>()?
        .into_iter()
        .flatten();
    Ok(iter::once(listing).chain(per_file).collect())
}

/// Every file under a directory, as a path relative to that directory.
fn relative_files(directory: &path::Path) -> Result<collections::BTreeSet<String>, Vec<String>> {
    files_under(directory)?
        .iter()
        .map(|file| {
            file.strip_prefix(directory)
                .map(|relative| relative.to_string_lossy().into_owned())
                .map_err(|e| vec![format!("{} escapes its directory: {e}", file.display())])
        })
        .collect()
}

/// Nothing when two XCFramework manifests agree once their library entries are sorted, or a note that they differ.
fn equal_xcframework_plists(
    tfc_file: &path::Path,
    copy_file: &path::Path,
) -> Result<Result<(), String>, Vec<String>> {
    let canonical =
        |file: &path::Path| workbench::read(file).map(|text| canonical_xcframework_plist(&text));
    Ok(if canonical(tfc_file)? == canonical(copy_file)? {
        Ok(())
    } else {
        Err("the manifests differ beyond the order of their library entries".to_string())
    })
}

/// An XCFramework manifest's text with the entries of its `AvailableLibraries` array in sorted order.
fn canonical_xcframework_plist(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let Some(array_open) = lines
        .iter()
        .position(|line| line.trim() == AVAILABLE_LIBRARIES_KEY)
        .map(|key| key + 1)
        .filter(|open| lines.get(*open).map(|line| line.trim()) == Some(PLIST_ARRAY_OPEN))
    else {
        return text.to_string();
    };
    let mut entries: Vec<Vec<&str>> = Vec::new();
    let mut entry: Vec<&str> = Vec::new();
    let mut depth = 0_usize;
    let mut array_close = None;
    for (position, line) in lines.iter().enumerate().skip(array_open + 1) {
        let tag = line.trim();
        if depth == 0 && tag == PLIST_ARRAY_CLOSE {
            array_close = Some(position);
            break;
        }
        entry.push(line);
        if tag == PLIST_DICT_OPEN {
            depth += 1;
        } else if tag == PLIST_DICT_CLOSE {
            depth = depth.saturating_sub(1);
        }
        if depth == 0 {
            entries.push(mem::take(&mut entry));
        }
    }
    let Some(array_close) = array_close else {
        return text.to_string();
    };
    entries.sort();
    lines[..=array_open]
        .iter()
        .copied()
        .chain(entries.into_iter().flatten())
        .chain(lines[array_close..].iter().copied())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Nothing when two files hold the same bytes, or a note that they differ.
fn equal_bytes(
    tfc_file: &path::Path,
    copy_file: &path::Path,
) -> Result<Result<(), String>, Vec<String>> {
    let read = |file: &path::Path| {
        fs::read(file).map_err(|e| vec![format!("cannot read {}: {e}", file.display())])
    };
    Ok(if read(tfc_file)? == read(copy_file)? {
        Ok(())
    } else {
        Err("the bytes differ".to_string())
    })
}

/// Nothing when two name sets are equal, or the names that only one side has.
fn set_difference(
    tfc_names: &collections::BTreeSet<String>,
    copy_names: &collections::BTreeSet<String>,
) -> Result<(), String> {
    let joined = |names: collections::btree_set::Difference<'_, String>| {
        names.cloned().collect::<Vec<_>>().join(", ")
    };
    match (
        joined(tfc_names.difference(copy_names)),
        joined(copy_names.difference(tfc_names)),
    ) {
        (only_tfc, only_copy) if only_tfc.is_empty() && only_copy.is_empty() => Ok(()),
        (only_tfc, only_copy) => Err(format!(
            "only at TFC: [{only_tfc}]; only in the copy: [{only_copy}]"
        )),
    }
}

/// The lines of a TOML table, from its header to the next table header.
fn toml_section(text: &str, header: &str) -> Option<String> {
    let from_header: Vec<&str> = text
        .lines()
        .skip_while(|line| line.trim() != header)
        .collect();
    let (first, rest) = from_header.split_first()?;
    Some(
        iter::once(*first)
            .chain(
                rest.iter()
                    .copied()
                    .take_while(|line| !line.trim_start().starts_with(TOML_TABLE_OPEN)),
            )
            .collect::<Vec<_>>()
            .join("\n")
            .trim_end()
            .to_string(),
    )
}

/// A check that a value declared at TFC equals the value declared in the copy.
fn declared_check(label: &str, tfc_value: Option<&str>, copy_value: Option<&str>) -> Check {
    Check {
        label: label.to_string(),
        outcome: match (tfc_value, copy_value) {
            (Some(tfc), Some(copy)) => equal_or_describe(tfc, copy),
            (None, _) => Err("not declared at TFC".to_string()),
            (_, None) => Err("not declared in the copy".to_string()),
        },
    }
}

/// The report of a gate whose checks all match, or the diagnostics of every check that differs.
fn verdict(gate: Gate, checks: &[Check]) -> Result<Vec<String>, Vec<String>> {
    let failures: Vec<String> = checks
        .iter()
        .filter_map(|check| match &check.outcome {
            Ok(()) => None,
            Err(difference) => Some(format!(
                "gate {} DIFFER  {}: {difference}",
                gate.name(),
                check.label
            )),
        })
        .collect();
    if failures.is_empty() {
        Ok(checks
            .iter()
            .map(|check| format!("gate {} MATCH  {}", gate.name(), check.label))
            .collect())
    } else {
        Err(failures)
    }
}

/// Nothing when two values are equal, or a description of both.
fn equal_or_describe(original: &str, copy: &str) -> Result<(), String> {
    if original == copy {
        Ok(())
    } else {
        Err(format!("TFC has {original}, the copy has {copy}"))
    }
}

/// Nothing when two texts are identical, or the first line at which they differ.
fn first_difference(tfc_text: &str, copy_text: &str) -> Result<(), String> {
    let longest = tfc_text.lines().count().max(copy_text.lines().count());
    padded_lines(tfc_text)
        .zip(padded_lines(copy_text))
        .take(longest)
        .enumerate()
        .find(|(_, (tfc_line, copy_line))| tfc_line != copy_line)
        .map_or(Ok(()), |(index, (tfc_line, copy_line))| {
            Err(format!(
                "line {}: TFC has {:?}, the copy has {:?}",
                index + FIRST_LINE_NUMBER,
                tfc_line.unwrap_or_default(),
                copy_line.unwrap_or_default()
            ))
        })
}

/// A text's lines, each wrapped in `Some`, followed by `None` forever.
fn padded_lines(text: &str) -> impl Iterator<Item = Option<&str>> {
    text.lines().map(Some).chain(iter::repeat(None))
}

/// Nothing when two file maps are identical, or the first path at which they differ.
fn first_file_difference(
    tfc_files: &collections::BTreeMap<path::PathBuf, Vec<u8>>,
    copy_files: &collections::BTreeMap<path::PathBuf, Vec<u8>>,
) -> Result<(), String> {
    let all_paths: collections::BTreeSet<&path::PathBuf> =
        tfc_files.keys().chain(copy_files.keys()).collect();
    all_paths
        .into_iter()
        .find_map(
            |relative| match (tfc_files.get(relative), copy_files.get(relative)) {
                (Some(tfc_bytes), Some(copy_bytes)) if tfc_bytes == copy_bytes => None,
                (Some(_), Some(_)) => Some(format!("{} differs", relative.display())),
                (Some(_), None) => Some(format!("{} exists only at TFC", relative.display())),
                (None, _) => Some(format!("{} exists only in the copy", relative.display())),
            },
        )
        .map_or(Ok(()), Err)
}

/// A check that every touched path is one that the given rule allows.
fn paths_check(label: &str, touched: &str, allowed: impl Fn(&str) -> bool) -> Check {
    Check {
        label: label.to_string(),
        outcome: match stray_paths(touched, allowed).as_slice() {
            [] => Ok(()),
            strays => Err(format!("also touches {}", strays.join(", "))),
        },
    }
}

/// The touched paths, one per line, that the given rule does not allow.
fn stray_paths(touched: &str, allowed: impl Fn(&str) -> bool) -> Vec<&str> {
    touched
        .lines()
        .filter(|touched_path| !allowed(touched_path))
        .collect()
}

/// Whether a path is one that the placement commit is meant to touch.
fn is_placement_path(touched: &str) -> bool {
    COPIED_MANIFESTS.contains(&touched) || PARENT_MANIFESTS.contains(&touched)
}

/// Whether a path is one that the branch may change relative to TAR.
fn is_branch_path(touched: &str, copied: &[&str]) -> bool {
    let under = |directory: &&str| {
        touched == *directory
            || touched
                .strip_prefix(directory)
                .is_some_and(|rest| rest.starts_with(PATH_SEPARATOR))
    };
    is_placement_path(touched) || copied.iter().any(under) || BRANCH_DIRECTORIES.iter().any(under)
}

/// A `cargo tree` text with every source annotation that starts with a given prefix removed.
fn erase_sources(tree: &str, source_prefixes: &[&str]) -> String {
    tree.lines()
        .map(|line| erase_line_sources(line, source_prefixes))
        .collect::<Vec<_>>()
        .join("\n")
}

/// One `cargo tree` line with every source annotation that starts with a given prefix removed.
fn erase_line_sources(line: &str, source_prefixes: &[&str]) -> String {
    match line.split_once(ANNOTATION_OPEN) {
        None => line.to_string(),
        Some((before, after)) => match after.split_once(ANNOTATION_CLOSE) {
            Some((annotation, rest))
                if source_prefixes
                    .iter()
                    .any(|prefix| annotation.starts_with(prefix)) =>
            {
                format!("{before}{}", erase_line_sources(rest, source_prefixes))
            }
            _ => format!(
                "{before}{ANNOTATION_OPEN}{}",
                erase_line_sources(after, source_prefixes)
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erases_only_annotations_with_a_named_prefix() {
        let prefixes = [ZINGOLIB_GIT_SOURCE, "/checkout"];
        assert_eq!(
            erase_line_sources(
                "1zingolib v6.0.0 (https://github.com/zingolabs/zingolib?rev=bd9e74ee0#bd9e74ee)",
                &prefixes
            ),
            "1zingolib v6.0.0"
        );
        assert_eq!(
            erase_line_sources("0zingo v2.0.0 (/checkout/zingo-ffi/lib)", &prefixes),
            "0zingo v2.0.0"
        );
        assert_eq!(
            erase_line_sources("2serde_derive v1.0.0 (proc-macro) (*)", &prefixes),
            "2serde_derive v1.0.0 (proc-macro) (*)"
        );
        assert_eq!(
            erase_line_sources(
                "2lightwallet-protocol v0.3.0 (https://github.com/zingolabs/lightwallet-protocol-rust?rev=9bdf#9bdf)",
                &prefixes
            ),
            "2lightwallet-protocol v0.3.0 (https://github.com/zingolabs/lightwallet-protocol-rust?rev=9bdf#9bdf)"
        );
    }

    #[test]
    fn first_difference_reports_the_first_differing_line() {
        assert_eq!(first_difference("a\nb", "a\nb"), Ok(()));
        assert!(first_difference("a\nb", "a\nc")
            .unwrap_err()
            .starts_with("line 2:"));
        assert!(first_difference("a", "a\nb").is_err());
    }

    #[test]
    fn placement_allows_only_its_intended_manifests() {
        let touched =
            "Cargo.toml\nzingo-ffi/Cargo.lock\nzingo-ffi/lib/Cargo.toml\nzingo-ffi/lib/src/lib.rs";
        assert_eq!(
            stray_paths(touched, is_placement_path),
            vec!["zingo-ffi/lib/Cargo.toml", "zingo-ffi/lib/src/lib.rs"]
        );
    }

    #[test]
    fn branch_paths_cover_the_copy_the_packaging_and_the_tooling_only() {
        let copied: Vec<&str> = COPIED_PATHS.iter().map(|copied| copied.copy).collect();
        let touched = "zingo-ffi/lib/src/lib.rs\nbindings/android/build.gradle.kts\ntools/workbench/src/lib.rs\nzingo-ffi/Cargo.lock\npepper-sync/src/sync.rs\nbindingsx/file\nzingolib/Cargo.toml";
        assert_eq!(
            stray_paths(touched, |path| is_branch_path(path, &copied)),
            vec![
                "pepper-sync/src/sync.rs",
                "bindingsx/file",
                "zingolib/Cargo.toml"
            ]
        );
    }

    #[test]
    fn file_maps_differ_by_content_or_presence() {
        let map = |entries: &[(&str, &[u8])]| {
            entries
                .iter()
                .map(|(name, bytes)| (path::PathBuf::from(name), bytes.to_vec()))
                .collect::<collections::BTreeMap<_, _>>()
        };
        let base = map(&[("zingo.kt", b"a")]);
        assert_eq!(first_file_difference(&base, &base), Ok(()));
        assert!(first_file_difference(&base, &map(&[("zingo.kt", b"b")])).is_err());
        assert!(first_file_difference(&base, &map(&[])).is_err());
    }

    #[test]
    fn parse_requires_every_flag_and_expands_all() {
        let args: Vec<String> = [
            "all",
            "--mobile",
            "/m",
            "--tfc",
            "t",
            "--tar",
            "r",
            "--import",
            "i",
            "--placement",
            "p",
        ]
        .map(String::from)
        .to_vec();
        let invocation = parse(&args).unwrap();
        assert_eq!(invocation.gates.len(), GATES.len());
        assert!(invocation.all);
        let (_, without_last) = args.split_last().unwrap();
        assert!(parse(without_last).is_err());
        assert!(parse(&["unknown".to_string()]).is_err());
    }

    #[test]
    fn declared_values_are_read_between_marker_and_terminator() {
        assert_eq!(
            TFC_MIN_SDK.find("        set(\"minSdkVersion\", 26)\n"),
            Some("26")
        );
        assert_eq!(
            AAR_MIN_SDK.find("<uses-sdk android:minSdkVersion=\"26\" />"),
            Some("26")
        );
        assert_eq!(
            JNA.find("    implementation(\"net.java.dev.jna:jna:5.18.1@aar\")"),
            Some("5.18.1@aar")
        );
        assert_eq!(JNA.find("no dependency here"), None);
    }

    #[test]
    fn toml_section_stops_at_the_next_table() {
        let manifest = "[workspace]\nmembers = []\n\n[profile.release]\nlto = \"thin\"\n\n[profile.test]\ndebug = 1\n";
        assert_eq!(
            toml_section(manifest, RELEASE_PROFILE_HEADER).as_deref(),
            Some("[profile.release]\nlto = \"thin\"")
        );
        assert_eq!(toml_section(manifest, "[patch.crates-io]"), None);
    }

    #[test]
    fn set_difference_names_both_sides() {
        let set = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<collections::BTreeSet<_>>()
        };
        assert_eq!(set_difference(&set(&["x86"]), &set(&["x86"])), Ok(()));
        assert_eq!(
            set_difference(&set(&["x86", "arm64-v8a"]), &set(&["x86", "x86_64"])),
            Err("only at TFC: [arm64-v8a]; only in the copy: [x86_64]".to_string())
        );
    }

    #[test]
    fn ffi_symbols_keep_c_names_and_drop_mangled_and_anonymous_names_and_headers() {
        let listing = "\nlibzingo.a(zingo-1a2b.o):\n_uniffi_zingo_fn_init\n__ZN5zingo4init17h0123456789abcdefE\n__RNvCs1_5zingo4init\n_anon.09f0247177e8ca7af6e80758e8c5d818.0.llvm.4913617149991479109\n_ffi_zingo_rustbuffer_free\n_uniffi_zingo_fn_init\n_anonymous_c_symbol\n";
        assert_eq!(
            ffi_symbols(listing),
            "anonymous_c_symbol\nffi_zingo_rustbuffer_free\nuniffi_zingo_fn_init"
        );
    }

    #[test]
    fn xcframework_plists_agree_across_library_entry_order_and_differ_on_content() {
        let manifest = |first: &str, second: &str| {
            format!(
                "<plist version=\"1.0\">\n<dict>\n\t<key>AvailableLibraries</key>\n\t<array>\n{first}\n{second}\n\t</array>\n\t<key>XCFrameworkFormatVersion</key>\n\t<string>1.0</string>\n</dict>\n</plist>\n"
            )
        };
        let entry = |identifier: &str| {
            format!(
                "\t\t<dict>\n\t\t\t<key>LibraryIdentifier</key>\n\t\t\t<string>{identifier}</string>\n\t\t\t<key>SupportedArchitectures</key>\n\t\t\t<array>\n\t\t\t\t<string>arm64</string>\n\t\t\t</array>\n\t\t</dict>"
            )
        };
        let device = entry("ios-arm64");
        let simulator = entry("ios-arm64_x86_64-simulator");
        assert_eq!(
            canonical_xcframework_plist(&manifest(&device, &simulator)),
            canonical_xcframework_plist(&manifest(&simulator, &device))
        );
        assert_ne!(
            canonical_xcframework_plist(&manifest(&device, &simulator)),
            canonical_xcframework_plist(&manifest(&device, &entry("ios-x86_64-simulator")))
        );
        assert_eq!(canonical_xcframework_plist("no manifest"), "no manifest");
    }

    #[test]
    fn embedded_descriptors_find_descriptors_but_not_the_function_name() {
        let bytes = b"\x00zm_2.0.24_320_f3d1a\x00junk zm_description17h00E\x00";
        assert_eq!(
            embedded_descriptors(bytes),
            ["zm_2.0.24_320_f3d1a".to_string()].into_iter().collect()
        );
    }

    #[test]
    fn tfc_android_files_cover_both_libraries_per_abi_and_both_kotlin_files() {
        let files = tfc_android_files();
        assert_eq!(
            files.len(),
            binding_layer::ANDROID_ABIS.len() * binding_layer::GENERATIONS.len()
                + binding_layer::GENERATIONS.len()
        );
        assert!(files
            .iter()
            .any(|(_, to)| to == "jniLibs/arm64-v8a/libuniffi_zingo.so"));
        assert!(files
            .iter()
            .any(|(_, to)| to == "kotlin/uniffi/zingo_nym_proxy_ffi/zingo_nym_proxy_ffi.kt"));
    }
}
