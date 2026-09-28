#![forbid(unsafe_code)]

use std::collections;
use std::env;
use std::fs;
use std::iter;
use std::path;

/// The program name that prefixes every diagnostic.
const PROGRAM: &str = "binding-copy-gate";

/// The invocation shape, reported when the arguments do not parse.
const USAGE: &str = "usage: binding-copy-gate <import|graph|bindings|all> \
    --mobile <zingo-mobile checkout at TFC> --tfc <rev> --import <rev> --placement <rev>";

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

/// The file names that a placement commit may touch.
const MANIFEST_FILE_NAMES: [&str; 2] = ["Cargo.toml", "Cargo.lock"];

/// The binding languages that zingo-mobile generates.
const LANGUAGES: [&str; 2] = ["kotlin", "swift"];

/// The scratch directory, under the zingolib root, that holds generated bindings.
const SCRATCH_DIR: &str = "target/binding-copy-gate";

/// The subdirectory of each side's scratch directory that holds cargo's build output.
const CARGO_TARGET_SUBDIR: &str = "cargo";

/// The cargo profile directory that a build without `--release` writes to.
const DEBUG_PROFILE_DIR: &str = "debug";

/// The proxy crate's library name, from which its dynamic library file name derives.
const PROXY_LIB_NAME: &str = "zingo_nym_proxy_ffi";

/// The binary in the wallet crate that generates bindings from the UDL file.
const WALLET_BINDGEN_BIN: &str = "uniffi-bindgen";

/// The package that holds the bindgen for library-mode generation.
const LIBRARY_BINDGEN_PACKAGE: &str = "zingo-uniffi-bindgen";

/// The binary that generates the proxy crate's bindings from its built library.
const LIBRARY_BINDGEN_BIN: &str = "zingo-uniffi-bindgen";

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

/// The binding set that gate 3 generates for one crate.
#[derive(Clone, Copy)]
enum Generation {
    /// The wallet crate's bindings, generated from its UDL file.
    Wallet,
    /// The proxy crate's bindings, generated from its built library.
    Proxy,
}

impl Generation {
    /// The prefix of this binding set's output directory names.
    fn label(self) -> &'static str {
        match self {
            Generation::Wallet => "wallet",
            Generation::Proxy => "proxy",
        }
    }
}

/// Every binding set that gate 3 generates.
const GENERATIONS: [Generation; 2] = [Generation::Wallet, Generation::Proxy];

/// One equivalence gate.
#[derive(Clone, Copy)]
enum Gate {
    /// Gate 1, which compares the imported object hashes with TFC's.
    Import,
    /// Gate 2, which checks the placement and compares resolved graphs.
    Graph,
    /// Gate 3, which compares the generated bindings.
    Bindings,
}

/// Every gate, in the order that the remedies assume.
const GATES: [Gate; 3] = [Gate::Import, Gate::Graph, Gate::Bindings];

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
        }
    }
}

/// The parsed command line.
struct Invocation<'a> {
    /// The gates to run, in order.
    gates: Vec<Gate>,
    /// The zingo-mobile checkout at TFC.
    mobile: path::PathBuf,
    /// The Freeze Commit.
    tfc: &'a str,
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
        root: invocation.mobile.clone(),
        layout: TFC_LAYOUT,
    };
    let copy_side = Side {
        name: "copy",
        root: workbench::repo_root()?,
        layout: COPY_LAYOUT,
    };
    require_checkout_at(&tfc_side.root, invocation.tfc)?;
    invocation
        .gates
        .iter()
        .map(|gate| run_gate(*gate, &invocation, &tfc_side, &copy_side))
        .collect::<Result<Vec<_>, _>>()
        .map(|reports| reports.concat())
}

/// Parse the gate selection and the four flags, all of which are required.
fn parse(args: &[String]) -> Result<Invocation<'_>, Vec<String>> {
    let (selection, flags) = args.split_first().ok_or_else(|| vec![USAGE.to_string()])?;
    let gates = if selection == ALL_GATES {
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
        mobile: path::PathBuf::from(required(MOBILE_FLAG)?),
        tfc: required(TFC_FLAG)?,
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

/// Gate 2: the placement touches only manifests, and each crate's resolved graph matches TFC's.
fn graph_checks(
    invocation: &Invocation,
    tfc_side: &Side,
    copy_side: &Side,
) -> Result<Vec<Check>, Vec<String>> {
    let touched = workbench::git(&[
        "-C",
        utf8(&copy_side.root)?,
        "diff",
        "--name-only",
        invocation.import,
        invocation.placement,
    ])?;
    let placement_check = Check {
        label: "placement touches only manifests and lockfiles".to_string(),
        outcome: match non_manifest_paths(&touched).as_slice() {
            [] => Ok(()),
            strays => Err(format!("also touches {}", strays.join(", "))),
        },
    };
    let source_prefixes = [
        ZINGOLIB_GIT_SOURCE,
        utf8(&tfc_side.root)?,
        utf8(&copy_side.root)?,
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
    Ok(iter::once(placement_check).chain(tree_checks).collect())
}

/// Gate 3: the bindings generated on each side match byte for byte.
fn binding_checks(tfc_side: &Side, copy_side: &Side) -> Result<Vec<Check>, Vec<String>> {
    let scratch = copy_side.file(SCRATCH_DIR);
    generate_bindings(tfc_side, &scratch)?;
    generate_bindings(copy_side, &scratch)?;
    binding_sets()
        .map(|(generation, language)| {
            let output = output_name(generation, language);
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
    if side_dir.exists() {
        fs::remove_dir_all(&side_dir)
            .map_err(|e| vec![format!("cannot clear {}: {e}", side_dir.display())])?;
    }
    let target_dir = side_dir.join(CARGO_TARGET_SUBDIR);
    let proxy_library = build_proxy_library(side, &target_dir)?;
    let wallet_crate = side.file(side.layout.wallet_crate);
    let udl = side.file(side.layout.udl);
    let wallet_workspace = side.file(side.layout.manifest(Workspace::Wallet));
    binding_sets().try_for_each(|(generation, language)| {
        let out_dir = side_dir.join(output_name(generation, language));
        let generate_args = match generation {
            Generation::Wallet => vec![
                "run",
                "--locked",
                "--manifest-path",
                utf8(&wallet_crate)?,
                "--target-dir",
                utf8(&target_dir)?,
                "--bin",
                WALLET_BINDGEN_BIN,
                "--",
                "generate",
                utf8(&udl)?,
            ],
            Generation::Proxy => vec![
                "run",
                "--locked",
                "--manifest-path",
                utf8(&wallet_workspace)?,
                "--target-dir",
                utf8(&target_dir)?,
                "--package",
                LIBRARY_BINDGEN_PACKAGE,
                "--bin",
                LIBRARY_BINDGEN_BIN,
                "--",
                "generate",
                "--library",
                utf8(&proxy_library)?,
            ],
        };
        let language_args = ["--language", language, "--out-dir", utf8(&out_dir)?];
        workbench::stdout_of(
            "cargo",
            &[generate_args.as_slice(), &language_args].concat(),
        )
        .map(drop)
    })
}

/// Build the proxy crate's dynamic library for the host and return its path.
fn build_proxy_library(side: &Side, target_dir: &path::Path) -> Result<path::PathBuf, Vec<String>> {
    workbench::stdout_of(
        "cargo",
        &[
            "build",
            "--locked",
            "--manifest-path",
            utf8(&side.file(side.layout.manifest(Workspace::Proxy)))?,
            "--target-dir",
            utf8(target_dir)?,
            "--lib",
        ],
    )?;
    Ok(target_dir.join(DEBUG_PROFILE_DIR).join(format!(
        "{}{PROXY_LIB_NAME}{}",
        env::consts::DLL_PREFIX,
        env::consts::DLL_SUFFIX
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
        utf8(&manifest)?,
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
        utf8(repository)?,
        "rev-parse",
        &format!("{rev}:{file}"),
    ])
    .map(|id| id.trim().to_string())
}

/// The full commit id that a rev names in a repository.
fn commit_id(repository: &path::Path, rev: &str) -> Result<String, Vec<String>> {
    workbench::git(&[
        "-C",
        utf8(repository)?,
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

/// A path as UTF-8, or a diagnostic naming it.
fn utf8(file: &path::Path) -> Result<&str, Vec<String>> {
    file.to_str()
        .ok_or_else(|| vec![format!("{} is not valid UTF-8", file.display())])
}

/// Every pairing of a binding set with a language, in a fixed order.
fn binding_sets() -> impl Iterator<Item = (Generation, &'static str)> {
    GENERATIONS
        .into_iter()
        .flat_map(|generation| LANGUAGES.map(|language| (generation, language)))
}

/// The name of the scratch directory that holds one binding set in one language.
fn output_name(generation: Generation, language: &str) -> String {
    format!("{}-{language}", generation.label())
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

/// The touched paths whose file names are neither a manifest nor a lockfile.
fn non_manifest_paths(touched: &str) -> Vec<&str> {
    touched
        .lines()
        .filter(|touched_path| {
            let file_name = path::Path::new(touched_path)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            !MANIFEST_FILE_NAMES.contains(&file_name)
        })
        .collect()
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
    fn manifest_paths_pass_and_others_are_named() {
        let touched = "Cargo.toml\nzingo-ffi/Cargo.lock\nzingo-ffi/lib/src/lib.rs";
        assert_eq!(
            non_manifest_paths(touched),
            vec!["zingo-ffi/lib/src/lib.rs"]
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
            "--import",
            "i",
            "--placement",
            "p",
        ]
        .map(String::from)
        .to_vec();
        let invocation = parse(&args).unwrap();
        assert_eq!(invocation.gates.len(), GATES.len());
        let (_, without_last) = args.split_last().unwrap();
        assert!(parse(without_last).is_err());
        assert!(parse(&["unknown".to_string()]).is_err());
    }
}
