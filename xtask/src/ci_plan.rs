use std::path::Path;

use crate::{TOOLCHAIN_FILE, git_in, read, stdout_from_stdin, toolchain_channel};

pub const RUNNER: &str = "ubuntu-24.04";

pub const IOS_RUNNER: &str = "warp-macos-15-arm64-12x";

pub const TEST_SHARDS: u32 = 16;

pub const IMAGE_NAME: &str = "ghcr.io/zingolabs/ci-build";

pub const ARTIFACTS_ENV_FILE: &str = ".env.testing-artifacts";

pub const IMAGE_DIR: &str = "docker-ci";

const TAG_LENGTH: usize = 14;

const ZAINO_IMAGE_TAG: &str = "ZAINO_IMAGE_TAG";
const ZEBRA_VERSION: &str = "ZEBRA_VERSION";
const NEXTEST_VERSION: &str = "NEXTEST_VERSION";

const USAGE: &str = "usage: cargo xtask ci-plan [image | image-tag]";

const GIT_HASH_OBJECT: &str = "hash-object";

/// - Runs `git hash-object` as child processes in `root`.
/// - Writes the plan, the image, or the image tag to stdout.
pub fn dispatch(root: &Path, args: &[String]) -> Result<(), Vec<String>> {
    let output = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => Plan::compute(root)?.json(),
        ["image"] => image(root)?,
        ["image-tag"] => image_tag(root)?,
        _ => return Err(vec![USAGE.to_string()]),
    };
    println!("{output}");
    Ok(())
}

pub struct Plan {
    pub runner: &'static str,
    pub ios_runner: &'static str,
    pub image: String,
    pub partitions: Vec<u32>,
}

impl Plan {
    /// - Runs `git hash-object` as child processes in `root`.
    pub fn compute(root: &Path) -> Result<Self, Vec<String>> {
        Ok(Self {
            runner: RUNNER,
            ios_runner: IOS_RUNNER,
            image: image(root)?,
            partitions: (1..=TEST_SHARDS).collect(),
        })
    }

    pub fn json(&self) -> String {
        let partitions = self
            .partitions
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "{{\"runner\":{},\"ios_runner\":{},\"image\":{},\"partitions\":[{partitions}]}}",
            json_string(self.runner),
            json_string(self.ios_runner),
            json_string(&self.image),
        )
    }
}

fn json_string(value: &str) -> String {
    let escaped: String = value
        .chars()
        .map(|c| match c {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            c if c.is_control() => format!("\\u{:04x}", c as u32),
            c => c.to_string(),
        })
        .collect();
    format!("\"{escaped}\"")
}

/// - Runs `git hash-object` as child processes in `root`.
pub fn image(root: &Path) -> Result<String, Vec<String>> {
    Ok(format!("{IMAGE_NAME}:{}", image_tag(root)?))
}

/// - Runs `git hash-object` as child processes in `root`.
pub fn image_tag(root: &Path) -> Result<String, Vec<String>> {
    let env = artifacts_env(root)?;
    let identity = format!(
        "RUST_{}-ZAINO_{}-ZEBRA_{}-NEXTEST_{}-CONTAINER_{}",
        toolchain_channel(root)?,
        env_value(&env, ZAINO_IMAGE_TAG)?,
        env_value(&env, ZEBRA_VERSION)?,
        env_value(&env, NEXTEST_VERSION)?,
        image_dir_hash(root)?,
    );
    short_hash(root, identity.as_bytes())
}

/// - Runs `git hash-object` as child processes in `root`.
/// - Reads the directory tree under `<root>/docker-ci` from disk.
fn image_dir_hash(root: &Path) -> Result<String, Vec<String>> {
    let mut paths = vec![ARTIFACTS_ENV_FILE.to_string(), TOOLCHAIN_FILE.to_string()];
    let mut image_files = files_under(root, IMAGE_DIR)?;
    image_files.sort();
    paths.extend(image_files);

    let mut listing = String::new();
    for path in &paths {
        let hash = git_in(root, &[GIT_HASH_OBJECT, path])?;
        listing.push_str(&format!("{path} {}\n", hash.trim()));
    }
    short_hash(root, listing.as_bytes())
}

/// - Runs `git hash-object --stdin` as a child process in `root`.
fn short_hash(root: &Path, content: &[u8]) -> Result<String, Vec<String>> {
    let hash = stdout_from_stdin(root, "git", &[GIT_HASH_OBJECT, "--stdin"], content)?;
    Ok(hash.trim()[..TAG_LENGTH].to_string())
}

/// - Reads the directory tree under `<root>/<relative>` from disk.
fn files_under(root: &Path, relative: &str) -> Result<Vec<String>, Vec<String>> {
    let mut files = Vec::new();
    let directory = root.join(relative);
    let entries = std::fs::read_dir(&directory)
        .map_err(|e| vec![format!("cannot read {}: {e}", directory.display())])?;
    for entry in entries {
        let entry = entry.map_err(|e| vec![format!("cannot read {}: {e}", directory.display())])?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| vec![format!("{} is not valid UTF-8", entry.path().display())])?;
        let child = format!("{relative}/{name}");
        let kind = entry
            .file_type()
            .map_err(|e| vec![format!("cannot read {}: {e}", entry.path().display())])?;
        if kind.is_dir() {
            files.extend(files_under(root, &child)?);
        } else if kind.is_file() {
            files.push(child);
        }
    }
    Ok(files)
}

/// - Reads `<root>/.env.testing-artifacts` from disk.
pub fn artifacts_env(root: &Path) -> Result<Vec<(String, String)>, Vec<String>> {
    Ok(parse_env(&read(&root.join(ARTIFACTS_ENV_FILE))?))
}

fn parse_env(contents: &str) -> Vec<(String, String)> {
    contents
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

pub fn env_value<'a>(env: &'a [(String, String)], key: &str) -> Result<&'a str, Vec<String>> {
    env.iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .ok_or_else(|| vec![format!("{key} is not set in {ARTIFACTS_ENV_FILE}")])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_lines_parse_and_comments_and_blanks_are_skipped() {
        let env = parse_env("# note\n\nA=1\n  B = two words \nbad line\n");
        assert_eq!(
            env,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "two words".to_string())
            ]
        );
        assert_eq!(env_value(&env, "B").unwrap(), "two words");
        assert!(env_value(&env, "C").is_err());
    }

    #[test]
    fn json_strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
    }

    #[test]
    fn the_plan_json_names_every_field() {
        let plan = Plan {
            runner: "r",
            ios_runner: "i",
            image: "img:tag".to_string(),
            partitions: vec![1, 2],
        };
        assert_eq!(
            plan.json(),
            "{\"runner\":\"r\",\"ios_runner\":\"i\",\"image\":\"img:tag\",\"partitions\":[1,2]}"
        );
    }

    #[test]
    fn the_partitions_count_from_one_to_the_shard_count() {
        let plan = Plan::compute(&crate::repo_root().unwrap()).unwrap();
        assert_eq!(plan.partitions.first(), Some(&1));
        assert_eq!(plan.partitions.len() as u32, TEST_SHARDS);
    }

    #[test]
    fn the_image_tag_is_fourteen_hex_characters() {
        let tag = image_tag(&crate::repo_root().unwrap()).unwrap();
        assert_eq!(tag.len(), TAG_LENGTH);
        assert!(tag.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
