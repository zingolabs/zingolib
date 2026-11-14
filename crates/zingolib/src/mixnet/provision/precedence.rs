use super::*;

fn resolved(path: &str, source: ProxySource) -> ProxyResolution {
    ProxyResolution {
        path: path.to_string(),
        source,
    }
}

#[test]
fn explicit_wins_over_everything() {
    let chosen = choose_proxy_path(
        Some("/explicit"),
        Some("/from-env".to_string()),
        Some("/bundled".to_string()),
        None,
    );
    assert_eq!(chosen, resolved("/explicit", ProxySource::Explicit));
}

#[test]
fn empty_explicit_counts_as_absent() {
    let chosen = choose_proxy_path(Some(""), Some("/from-env".to_string()), None, None);
    assert_eq!(chosen, resolved("/from-env", ProxySource::Environment));
}

#[test]
fn env_wins_over_bundled() {
    let chosen = choose_proxy_path(
        None,
        Some("/from-env".to_string()),
        Some("/bundled".to_string()),
        None,
    );
    assert_eq!(chosen, resolved("/from-env", ProxySource::Environment));
}

#[test]
fn empty_env_counts_as_absent() {
    let chosen = choose_proxy_path(
        None,
        Some(String::new()),
        Some("/bundled".to_string()),
        None,
    );
    assert_eq!(chosen, resolved("/bundled", ProxySource::Bundled));
}

#[test]
fn bundled_wins_over_bare_name() {
    let chosen = choose_proxy_path(None, None, Some("/bundled".to_string()), None);
    assert_eq!(chosen, resolved("/bundled", ProxySource::Bundled));
}

#[test]
fn bare_name_is_the_final_fallback() {
    let searched = PathBuf::from("/opt/wallet");
    let chosen = choose_proxy_path(None, None, None, Some(searched.clone()));
    assert_eq!(
        chosen,
        resolved(
            NYM_PROXY_BINARY_NAME,
            ProxySource::PathLookup {
                bundled_dir: Some(searched),
            },
        )
    );
}

#[test]
fn the_fallback_trail_names_every_source_that_was_empty() {
    let chosen = choose_proxy_path(None, None, None, Some(PathBuf::from("/opt/wallet")));
    let trail = chosen.to_string();
    assert!(
        trail.starts_with("'nym-proxy' (left to the PATH lookup"),
        "{trail}"
    );
    assert!(trail.contains("no explicit path was given"), "{trail}");
    assert!(trail.contains("$ZINGO_NYM_PROXY is unset"), "{trail}");
    assert!(
        trail.contains("no nym-proxy is bundled in '/opt/wallet'"),
        "{trail}"
    );
}

#[test]
fn a_found_source_renders_the_path_and_its_origin() {
    let chosen = choose_proxy_path(None, Some("/from-env".to_string()), None, None);
    assert_eq!(chosen.to_string(), "'/from-env' (from $ZINGO_NYM_PROXY)");
}

#[test]
fn bundled_lookup_requires_the_file_to_exist() {
    let dir = tempfile::tempdir().expect("a scratch directory");
    assert_eq!(bundled_proxy_path(Some(dir.path())), None);

    let name = format!("{NYM_PROXY_BINARY_NAME}{}", std::env::consts::EXE_SUFFIX);
    std::fs::write(dir.path().join(&name), b"").expect("touch the candidate");
    let found = bundled_proxy_path(Some(dir.path())).expect("the candidate is found");
    assert!(found.ends_with(&name));
}

#[test]
fn no_bundled_dir_hint_means_no_bundled_candidate() {
    assert_eq!(bundled_proxy_path(None), None);
}
