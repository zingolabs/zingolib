use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub const BINARY: &str = "orasust";
pub const VERSION_FLAG: &str = "--orasust-version";
const MANIFEST: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"));
const PIN_PATH: [&str; 4] = ["package", "metadata", "orasust", "version"];
const INSTALL: &str = "cargo install --locked";
const CRATE_VERSION_SEPARATOR: char = '@';
const VERSION_COMMAND: &str = "version";
const VERSION_LABEL: &str = "Version:";
const PUSH: &str = "push";
const RESOLVE: &str = "resolve";
const MANIFEST_FETCH: [&str; 2] = ["manifest", "fetch"];
const ANNOTATION_FLAG: &str = "--annotation";
const USERNAME_FLAG: &str = "--username";
const PASSWORD_STDIN_FLAG: &str = "--password-stdin";
const DIGEST_LABEL: &str = "Digest:";
const ANNOTATION_SEPARATOR: char = '=';
const MEDIA_TYPE_SEPARATOR: char = ':';
const QUOTE: char = '"';
const KEY_VALUE_SEPARATOR: char = ':';
const CURRENT_DIR: &str = ".";
const STDOUT: &str = "stdout";
const STDERR: &str = "stderr";
const CREDENTIAL_NOT_FOUND: &str = "basic credential not found";
const NOT_FOUND: &str = "not found";

pub fn version() -> Result<String, Vec<String>> {
    let manifest: toml_edit::DocumentMut = MANIFEST
        .parse()
        .map_err(|error| vec![format!("workbench Cargo.toml does not parse: {error}")])?;
    let pin = PIN_PATH
        .iter()
        .try_fold(manifest.as_item(), |item, key| item.get(key))
        .and_then(|item| item.as_str())
        .ok_or_else(|| {
            vec![format!(
                "workbench Cargo.toml names no string at {}",
                PIN_PATH.join(".")
            )]
        })?;
    Ok(pin.to_string())
}

pub fn install_command(version: &str) -> String {
    format!("{INSTALL} {BINARY}{CRATE_VERSION_SEPARATOR}{version}")
}

fn install_hint(version: &str) -> String {
    format!("install it with `{}`", install_command(version))
}

pub fn installed_version(report: &str) -> Option<&str> {
    report
        .lines()
        .find_map(|line| line.strip_prefix(VERSION_LABEL))
        .map(str::trim)
}

/// - Runs `orasust version` as a child process.
pub fn pinned() -> Result<(), Vec<String>> {
    let pin = version()?;
    let report = crate::stdout_of(BINARY, &[VERSION_COMMAND])
        .map_err(|_| vec![format!("{BINARY} is not installed"), install_hint(&pin)])?;
    match installed_version(&report) {
        Some(installed) if installed == pin => Ok(()),
        installed => Err(vec![
            format!(
                "{BINARY} {} is installed and {pin} is pinned",
                installed.unwrap_or_default()
            ),
            install_hint(&pin),
        ]),
    }
}

pub fn pushed_digest(report: &str) -> Result<String, Vec<String>> {
    report
        .lines()
        .find_map(|line| line.strip_prefix(DIGEST_LABEL))
        .map(|digest| digest.trim().to_string())
        .ok_or_else(|| {
            vec![format!(
                "{BINARY} {PUSH} printed no {DIGEST_LABEL} line: {report}"
            )]
        })
}

pub fn annotation<'a>(manifest: &'a str, key: &str) -> Option<&'a str> {
    let quoted_key = format!("{QUOTE}{key}{QUOTE}");
    let after_key = &manifest[manifest.find(&quoted_key)? + quoted_key.len()..];
    let after_colon = after_key.trim_start().strip_prefix(KEY_VALUE_SEPARATOR)?;
    let value = after_colon.trim_start().strip_prefix(QUOTE)?;
    Some(&value[..value.find(QUOTE)?])
}

pub struct Credentials {
    pub username: String,
    pub token: String,
}

pub struct Outcome {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// - Runs `orasust <args>` as a child process in `dir`, with `token` written to its stdin when
///   one is given, and with its stdout and stderr captured.
fn outcome(dir: &Path, args: &[&str], token: Option<&str>) -> Result<Outcome, Vec<String>> {
    let mut command = Command::new(BINARY);
    command
        .args(args)
        .current_dir(dir)
        .stdin(if token.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| vec![format!("failed to run {BINARY}: {e}")])?;
    if let Some(token) = token {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| vec![format!("{BINARY} took no stdin")])?;
        stdin
            .write_all(token.as_bytes())
            .map_err(|e| vec![format!("cannot hand {BINARY} the token: {e}")])?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| vec![format!("failed to wait for {BINARY}: {e}")])?;
    let text = |bytes: Vec<u8>, stream: &str| {
        String::from_utf8(bytes).map_err(|e| vec![format!("{BINARY} {stream} not utf-8: {e}")])
    };
    Ok(Outcome {
        success: output.status.success(),
        stdout: text(output.stdout, STDOUT)?,
        stderr: text(output.stderr, STDERR)?,
    })
}

/// - Runs `orasust <args>` through [`outcome`] and writes its stderr to this process's stderr.
fn stdout_or_failure(
    dir: &Path,
    args: &[&str],
    token: Option<&str>,
) -> Result<String, Vec<String>> {
    let outcome = outcome(dir, args, token)?;
    eprint!("{}", outcome.stderr);
    if !outcome.success {
        return Err(vec![format!("`{BINARY} {}` failed", args.join(" "))]);
    }
    Ok(outcome.stdout)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution {
    Found(String),
    Absent,
    Private,
}

pub fn resolution(outcome: &Outcome) -> Result<Resolution, Vec<String>> {
    if outcome.success {
        return Ok(Resolution::Found(outcome.stdout.trim().to_string()));
    }
    if outcome.stderr.contains(CREDENTIAL_NOT_FOUND) {
        return Ok(Resolution::Private);
    }
    if outcome.stderr.contains(NOT_FOUND) {
        return Ok(Resolution::Absent);
    }
    let lines: Vec<String> = outcome.stderr.lines().map(str::to_string).collect();
    Err(if lines.is_empty() {
        vec![format!("`{BINARY} {RESOLVE}` failed without a message")]
    } else {
        lines
    })
}

/// - Runs `orasust push` as a child process through [`stdout_or_failure`].
pub fn push(
    reference: &str,
    file: &Path,
    media_type: &str,
    annotations: &[(String, String)],
    credentials: &Credentials,
) -> Result<String, Vec<String>> {
    let pairs: Vec<String> = annotations
        .iter()
        .map(|(key, value)| format!("{key}{ANNOTATION_SEPARATOR}{value}"))
        .collect();
    let name = crate::file_name_of(file)?;
    let layer = format!("{name}{MEDIA_TYPE_SEPARATOR}{media_type}");
    let mut args = vec![
        PUSH,
        USERNAME_FLAG,
        &credentials.username,
        PASSWORD_STDIN_FLAG,
    ];
    for pair in &pairs {
        args.push(ANNOTATION_FLAG);
        args.push(pair);
    }
    args.push(reference);
    args.push(&layer);
    let dir = file.parent().unwrap_or_else(|| Path::new(CURRENT_DIR));
    pushed_digest(&stdout_or_failure(dir, &args, Some(&credentials.token))?)
}

/// - Runs `orasust resolve` as a child process, anonymously, through [`outcome`].
pub fn resolve(reference: &str) -> Result<Resolution, Vec<String>> {
    resolution(&outcome(
        Path::new(CURRENT_DIR),
        &[RESOLVE, reference],
        None,
    )?)
}

/// - Runs `orasust resolve` as a child process with the credentials, through [`outcome`].
pub fn resolve_as(reference: &str, credentials: &Credentials) -> Result<Resolution, Vec<String>> {
    let args = [
        RESOLVE,
        USERNAME_FLAG,
        &credentials.username,
        PASSWORD_STDIN_FLAG,
        reference,
    ];
    resolution(&outcome(
        Path::new(CURRENT_DIR),
        &args,
        Some(&credentials.token),
    )?)
}

/// - Runs `orasust manifest fetch` as a child process through [`stdout_or_failure`].
pub fn manifest(reference: &str) -> Result<String, Vec<String>> {
    let args = [MANIFEST_FETCH.as_slice(), &[reference]].concat();
    stdout_or_failure(Path::new(CURRENT_DIR), &args, None)
}

#[cfg(test)]
mod tests {
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
}
