#[test]
fn files_under_walks_every_directory_but_the_skipped_ones_in_sorted_order(
) -> Result<(), Vec<String>> {
    let root = std::env::temp_dir().join(format!(
        "workbench-files-under-{}-{}",
        std::process::id(),
        line!()
    ));
    let io = |error: std::io::Error| vec![format!("{}: {error}", root.display())];
    std::fs::create_dir_all(root.join("src/deep")).map_err(io)?;
    std::fs::create_dir_all(root.join("target/debug")).map_err(io)?;
    std::fs::write(root.join("src/deep/b.rs"), "").map_err(io)?;
    std::fs::write(root.join("src/a.rs"), "").map_err(io)?;
    std::fs::write(root.join("README.md"), "").map_err(io)?;
    std::fs::write(root.join("target/debug/generated.rs"), "").map_err(io)?;
    let files = super::files_under(&root, &["target"])?;
    std::fs::remove_dir_all(&root).map_err(io)?;
    assert_eq!(
        files,
        [
            root.join("README.md"),
            root.join("src/a.rs"),
            root.join("src/deep/b.rs"),
        ]
    );
    assert!(
        super::files_under(&root, &[]).is_err(),
        "a missing root is an error"
    );
    Ok(())
}

#[test]
fn a_warning_is_a_workflow_command_only_under_actions() {
    assert_eq!(
        super::warning_line("base is gone", true),
        "::warning::base is gone"
    );
    assert_eq!(
        super::warning_line("base is gone", false),
        "warning: base is gone"
    );
}

use super::*;

/// A workbench crate directory under a checkout that was moved or removed after the build.
const ABSENT_WORKBENCH_DIR: &str = "/absent/zingolib/tools/workbench";

/// - Runs the same child processes as [`ManifestLookup::of`] over the repo root.
fn manifest_above(dir: &Path) -> Result<PathBuf, Vec<String>> {
    ManifestLookup::of(&repo_root()?)?
        .nearest(dir)
        .map(|(manifest, _)| manifest)
}

#[test]
fn the_root_is_two_directories_above_the_workbench_crate() -> Result<(), Vec<String>> {
    let workbench_dir = Path::new(BUILT_WORKBENCH_DIR);
    let tools_dir = required(workbench_dir.parent(), "parent of the workbench directory")?;
    assert_eq!(
        root_above(workbench_dir)?,
        required(tools_dir.parent(), "parent of the tools directory")?
    );
    Ok(())
}

#[test]
fn repo_root_is_the_root_above_the_built_crate_directory() -> Result<(), Vec<String>> {
    assert_eq!(repo_root()?, root_above(Path::new(BUILT_WORKBENCH_DIR))?);
    Ok(())
}

#[test]
fn a_foreign_manifest_directory_in_the_environment_does_not_move_the_root(
) -> Result<(), Vec<String>> {
    let test_binary =
        std::env::current_exe().map_err(|e| vec![format!("cannot locate the test binary: {e}")])?;
    let inner = Command::new(test_binary)
        .env("CARGO_MANIFEST_DIR", ABSENT_WORKBENCH_DIR)
        .args([
            "--exact",
            "tests::repo_root_is_the_root_above_the_built_crate_directory",
        ])
        .stdout(Stdio::null())
        .status()
        .map_err(|e| vec![format!("cannot rerun the test binary: {e}")])?;
    assert!(inner.success());
    Ok(())
}

#[test]
fn a_directory_that_is_not_the_workbench_crate_is_refused_by_name() -> Result<(), Vec<String>> {
    let source_dir = Path::new(BUILT_WORKBENCH_DIR).join("src");
    let diagnostic = refused(root_above(&source_dir))?.concat();
    assert!(diagnostic.contains(utf8(&source_dir)?));
    Ok(())
}

#[test]
fn a_workbench_crate_directory_that_is_absent_is_refused_by_name() -> Result<(), Vec<String>> {
    let diagnostic = refused(root_above(Path::new(ABSENT_WORKBENCH_DIR)))?.concat();
    assert!(diagnostic.contains(ABSENT_WORKBENCH_DIR));
    Ok(())
}

#[test]
fn channel_value_recognises_only_quoted_assignments() {
    assert_eq!(
        channel_value("channel = \"1.91.0\"").as_deref(),
        Some("1.91.0")
    );
    assert_eq!(channel_value("  channel=\"1.91\"").as_deref(), Some("1.91"));
    assert_eq!(channel_value("# channel = \"x\""), None);
    assert_eq!(channel_value("components = [\"clippy\"]"), None);
    assert_eq!(channel_value("[toolchain]"), None);
}

#[test]
fn parses_separate_and_joined_dest() -> Result<(), Vec<String>> {
    let sep = vec![
        "--release".to_string(),
        "--dest".to_string(),
        "/x".to_string(),
    ];
    assert_eq!(parse_dest(&sep)?, Some(PathBuf::from("/x")));

    let joined = vec!["--dest=/y".to_string()];
    assert_eq!(parse_dest(&joined)?, Some(PathBuf::from("/y")));
    Ok(())
}

#[test]
fn no_dest_is_none() -> Result<(), Vec<String>> {
    assert_eq!(parse_dest(&["--release".to_string()])?, None);
    Ok(())
}

#[test]
fn dest_without_value_is_an_error() {
    assert!(parse_dest(&["--dest".to_string()]).is_err());
}

#[test]
fn the_manifest_above_a_nested_member_source_is_that_member() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    assert_eq!(
        manifest_above(&root.join("ffi/lib/src"))?,
        root.join("ffi/lib").join(MANIFEST)
    );
    assert_eq!(manifest_above(&root.join("docs"))?, root.join(MANIFEST));
    Ok(())
}

#[test]
fn a_virtual_manifest_declares_no_package() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    assert!(!declares_a_package(&root.join(MANIFEST))?);
    assert!(declares_a_package(&root.join("bin/cli").join(MANIFEST))?);
    Ok(())
}

#[test]
fn the_root_workspace_members_include_the_nested_ones() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    let members = workspace_members(&root.join(MANIFEST))?;
    for member in ["ffi/lib", "ffi/uniffi-bindgen", "bin/cli"] {
        assert!(
            members.contains(&root.join(member).join(MANIFEST)),
            "{member}"
        );
    }
    Ok(())
}

#[test]
fn every_workspace_is_found_by_its_lockfile() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    let found = workspace_manifests(&root)?;
    for workspace in ["", WORKBENCH_RELATIVE_DIR] {
        assert!(
            found.contains(&root.join(workspace).join(MANIFEST)),
            "{workspace:?}"
        );
    }
    Ok(())
}

const VERIFIER_SHIM: &str = "rustls-platform-verifier";
const PACKAGE_KEY: &str = "package";
const NAME_KEY: &str = "name";
const SOURCE_KEY: &str = "source";

#[test]
fn the_verifier_shim_patch_is_in_effect() -> Result<(), Vec<String>> {
    let lockfile = repo_root()?.join(LOCKFILE);
    let text = at_path(&lockfile, "read", |path| std::fs::read_to_string(path))?;
    let document: toml_edit::DocumentMut = text
        .parse()
        .map_err(|error| vec![format!("{} does not parse: {error}", lockfile.display())])?;
    let packages = document
        .get(PACKAGE_KEY)
        .and_then(|item| item.as_array_of_tables())
        .ok_or_else(|| {
            vec![format!(
                "{} holds no [[package]] tables",
                lockfile.display()
            )]
        })?;
    let verifiers: Vec<&toml_edit::Table> = packages
        .iter()
        .filter(|package| {
            package.get(NAME_KEY).and_then(|name| name.as_str()) == Some(VERIFIER_SHIM)
        })
        .collect();
    let shim = match verifiers.as_slice() {
        [shim] => shim,
        other => {
            return Err(vec![format!(
                "the lockfile holds {} entries for {VERIFIER_SHIM}, not one: the zingo-adrs zingolib/0021 patch table is not in effect",
                other.len()
            )]);
        }
    };
    assert!(
        shim.get(SOURCE_KEY).is_none(),
        "the lockfile resolves {VERIFIER_SHIM} from a registry, so the zingo-adrs zingolib/0021 patch table is not in effect"
    );
    Ok(())
}

#[test]
fn a_deleted_file_resolves_through_its_nearest_existing_directory() -> Result<(), Vec<String>> {
    let root = repo_root()?;
    assert_eq!(
        existing_dir_of(&root, Path::new("bin/cli/src/gone/file.rs")),
        root.join("bin/cli/src")
    );
    assert_eq!(existing_dir_of(&root, Path::new("README.md")), root);
    Ok(())
}

#[test]
fn numeric_validation_matches_x_y_z() {
    assert!(is_concrete_numeric("1.91.0"));
    assert!(is_concrete_numeric("1.91"));
    assert!(!is_concrete_numeric("stable"));
    assert!(!is_concrete_numeric("nightly"));
    assert!(!is_concrete_numeric("1"));
    assert!(!is_concrete_numeric("1.91.0.1"));
    assert!(!is_concrete_numeric("1..0"));
}
