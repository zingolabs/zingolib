#![forbid(unsafe_code)]

pub const LENGTH: usize = 16;
pub const FULL_LENGTH: usize = 40;

pub fn abbreviate(commit: &str) -> Option<&str> {
    let full = commit.len() == FULL_LENGTH
        && commit
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    commit.get(..LENGTH).filter(|_| full)
}
