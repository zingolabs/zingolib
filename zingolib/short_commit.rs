#![forbid(unsafe_code)]

pub const LENGTH: usize = 16;
pub const FULL_LENGTH: usize = 40;

pub fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn abbreviate(commit: &str) -> Option<&str> {
    commit.get(..LENGTH).filter(|_| is_hex(commit, FULL_LENGTH))
}
