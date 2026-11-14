use super::*;

#[test]
fn the_version_line_is_read_from_the_version_report() {
    assert_eq!(
        installed_version("Version:        0.1.0\nOS/Arch:        linux/x86_64\n"),
        Some("0.1.0")
    );
    assert_eq!(installed_version("orasust 0.1.0"), None);
}

#[test]
fn the_pushed_digest_is_the_digest_line_of_the_push_report() {
    let report = "Uploaded  d5bd64a5c43b zingo.aar\nPushed [registry] ghcr.io/x/y:z\n\
                      ArtifactType: application/vnd.unknown.artifact.v1\nDigest: sha256:abc\n";
    assert_eq!(pushed_digest(report), Ok("sha256:abc".to_string()));
    assert!(pushed_digest("Pushed [registry] ghcr.io/x/y:z\n").is_err());
}

#[test]
fn an_annotation_is_read_from_the_manifest_as_the_registry_serves_it() {
    let compact = r#"{"schemaVersion":2,"annotations":{"org.opencontainers.image.created":"2026-10-09T20:51:30Z","org.opencontainers.image.revision":"abc"}}"#;
    assert_eq!(
        annotation(compact, "org.opencontainers.image.revision"),
        Some("abc")
    );
    let pretty =
        "{\n  \"annotations\": {\n    \"org.opencontainers.image.revision\" : \"abc\"\n  }\n}";
    assert_eq!(
        annotation(pretty, "org.opencontainers.image.revision"),
        Some("abc")
    );
    assert_eq!(
        annotation(compact, "org.zingolabs.zingolib.descriptor"),
        None
    );
}

fn failed(stderr: &str) -> Outcome {
    Outcome {
        success: false,
        stdout: String::new(),
        stderr: stderr.to_string(),
    }
}

#[test]
fn a_resolve_is_found_absent_private_or_a_failure_by_what_the_client_printed(
) -> Result<(), Vec<String>> {
    let found = Outcome {
        success: true,
        stdout: "sha256:abc\n".to_string(),
        stderr: String::new(),
    };
    assert_eq!(
        resolution(&found),
        Ok(Resolution::Found("sha256:abc".to_string()))
    );
    assert_eq!(
        resolution(&failed(
            "Error response from registry: failed to resolve digest: r.io/a:b: not found\n"
        )),
        Ok(Resolution::Absent)
    );
    assert_eq!(
        resolution(&failed("Error: basic credential not found\n")),
        Ok(Resolution::Private)
    );
    let unavailable = "Error response from registry: \"https://r.io/v2/a/manifests/b\": response status code 503: Service Unavailable\n";
    let diagnostic = crate::refused(resolution(&failed(unavailable)))?.concat();
    assert!(diagnostic.contains("503"), "{diagnostic}");
    assert!(!crate::refused(resolution(&failed("")))?.is_empty());
    Ok(())
}

#[test]
fn the_pin_is_read_from_the_workbench_manifest() -> Result<(), Vec<String>> {
    let pin = version()?;
    let components: Vec<&str> = pin.split('.').collect();
    assert_eq!(components.len(), 3, "{pin} is not a release version");
    assert!(components.iter().all(|c| c.parse::<u64>().is_ok()), "{pin}");
    Ok(())
}

#[test]
fn the_install_command_names_the_pinned_version() {
    assert_eq!(
        install_command("0.1.3"),
        "cargo install --locked orasust@0.1.3"
    );
}
