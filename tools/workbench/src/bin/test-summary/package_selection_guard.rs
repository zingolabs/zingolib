use super::*;

#[test]
fn rejects_every_selection_flag_form() {
    for arg in [
        "-p",
        "--package",
        "--package=zingolib",
        "--workspace",
        "--all",
        "--exclude",
        "--exclude=zingolib-cli",
        "--manifest-path",
        "--manifest-path=Cargo.toml",
    ] {
        assert!(is_package_selection_arg(arg), "should reject {arg}");
    }
}

/// Phases must select tests (filtersets), never packages: a package
/// selection re-unifies features per phase and compiles a distinct
/// zingolib variant per phase, defeating the shared --workspace
/// build scope.
#[test]
fn phase_invocations_select_tests_not_packages() {
    for (_, selection) in PHASES.iter() {
        for arg in selection.iter() {
            assert!(
                !is_package_selection_arg(arg),
                "phase selection {selection:?} carries package-selection arg {arg:?}"
            );
        }
    }
}

#[test]
fn passes_ordinary_nextest_args() {
    for arg in [
        "--no-fail-fast",
        "--no-capture",
        "-E",
        "test(slow)",
        "some_test_name",
        "--run-ignored",
    ] {
        assert!(!is_package_selection_arg(arg), "should pass {arg}");
    }
}
