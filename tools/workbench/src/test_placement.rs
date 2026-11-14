const TEST_ATTRIBUTE_PATHS: [&str; 2] = ["test", "tokio::test"];

const TEST_MACRO_PATHS: [&str; 2] = ["proptest", "proptest::proptest"];

// Keywords that open an item other than a function, and so end the reach
// of the attributes read since the last item.
const ITEM_KEYWORDS: [&str; 10] = [
    "struct",
    "enum",
    "union",
    "trait",
    "impl",
    "use",
    "static",
    "type",
    "let",
    "macro_rules",
];

// Where a test function sits in a source file, as nextest's naming of an
// integration test sees it: only a top-level test keeps its bare name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestPlacement {
    TopLevel,
    Nested,
    Unattributed,
    Absent,
}

// One `fn` item a Rust source defines. `modules` holds the enclosing
// modules, outermost first, and `enclosing_bodies` counts the braces open
// around the item that are not modules (function bodies, impl blocks and
// other blocks), inside which the test harness collects nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    pub modules: Vec<String>,
    pub enclosing_bodies: usize,
    pub test_attribute: bool,
}

impl Function {
    pub fn is_harness_test(&self) -> bool {
        self.test_attribute && self.enclosing_bodies == 0
    }

    pub fn nextest_name(&self) -> String {
        self.modules
            .iter()
            .map(String::as_str)
            .chain([self.name.as_str()])
            .collect::<Vec<_>>()
            .join("::")
    }

    pub fn placement(&self) -> TestPlacement {
        let top_level = self.modules.is_empty() && self.enclosing_bodies == 0;
        match (self.test_attribute, top_level) {
            (false, _) => TestPlacement::Unattributed,
            (true, true) => TestPlacement::TopLevel,
            (true, false) => TestPlacement::Nested,
        }
    }
}

// The lexical region the blanking pass is inside.
enum Region {
    Code,
    LineComment,
    BlockComment(usize),
    Str,
    RawStr(usize),
    Char,
}

pub(crate) fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

// The length of a raw string opener (`r"`, `r#"`, `br##"`) at `start`,
// with its hash count, or nothing when no raw string opens there.
fn raw_string_opener(chars: &[char], start: usize) -> Option<(usize, usize)> {
    let mut at = start;
    if chars.get(at) == Some(&'b') {
        at += 1;
    }
    if chars.get(at) != Some(&'r') {
        return None;
    }
    at += 1;
    let hashes = chars[at..].iter().take_while(|&&c| c == '#').count();
    at += hashes;
    (chars.get(at) == Some(&'"')).then_some((at + 1 - start, hashes))
}

// Whether a `'` at `at` opens a character literal rather than a lifetime.
fn opens_character_literal(chars: &[char], at: usize) -> bool {
    matches!(
        (chars.get(at + 1), chars.get(at + 2)),
        (Some('\\'), _) | (Some(_), Some('\''))
    )
}

// The source with every comment, string and character literal replaced by
// spaces, newlines kept, so the item pass sees code alone.
fn code_only(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut region = Region::Code;
    let mut at = 0;
    let blank = |c: char| if c == '\n' { '\n' } else { ' ' };
    while at < chars.len() {
        let c = chars[at];
        let next = chars.get(at + 1).copied();
        match region {
            Region::Code => {
                let previous_is_identifier = at > 0 && is_identifier_character(chars[at - 1]);
                if c == '/' && next == Some('/') {
                    region = Region::LineComment;
                    out.push_str("  ");
                    at += 2;
                } else if c == '/' && next == Some('*') {
                    region = Region::BlockComment(1);
                    out.push_str("  ");
                    at += 2;
                } else if let Some((length, hashes)) = (!previous_is_identifier)
                    .then(|| raw_string_opener(&chars, at))
                    .flatten()
                {
                    region = Region::RawStr(hashes);
                    out.extend(std::iter::repeat_n(' ', length));
                    at += length;
                } else if c == '"' {
                    region = Region::Str;
                    out.push(' ');
                    at += 1;
                } else if c == '\'' && opens_character_literal(&chars, at) {
                    region = Region::Char;
                    out.push(' ');
                    at += 1;
                } else {
                    out.push(c);
                    at += 1;
                }
            }
            Region::LineComment => {
                if c == '\n' {
                    region = Region::Code;
                }
                out.push(blank(c));
                at += 1;
            }
            Region::BlockComment(depth) => {
                if c == '*' && next == Some('/') {
                    region = if depth == 1 {
                        Region::Code
                    } else {
                        Region::BlockComment(depth - 1)
                    };
                    out.push_str("  ");
                    at += 2;
                } else if c == '/' && next == Some('*') {
                    region = Region::BlockComment(depth + 1);
                    out.push_str("  ");
                    at += 2;
                } else {
                    out.push(blank(c));
                    at += 1;
                }
            }
            Region::Str => {
                if c == '\\' {
                    out.push(' ');
                    out.push(next.map_or(' ', blank));
                    at += 2;
                } else {
                    if c == '"' {
                        region = Region::Code;
                    }
                    out.push(blank(c));
                    at += 1;
                }
            }
            Region::RawStr(hashes) => {
                let closes = c == '"'
                    && chars[at + 1..]
                        .iter()
                        .take(hashes)
                        .filter(|&&h| h == '#')
                        .count()
                        == hashes;
                if closes {
                    region = Region::Code;
                    out.extend(std::iter::repeat_n(' ', hashes + 1));
                    at += hashes + 1;
                } else {
                    out.push(blank(c));
                    at += 1;
                }
            }
            Region::Char => {
                if c == '\\' {
                    out.push(' ');
                    out.push(next.map_or(' ', blank));
                    at += 2;
                } else {
                    if c == '\'' {
                        region = Region::Code;
                    }
                    out.push(blank(c));
                    at += 1;
                }
            }
        }
    }
    out
}

// The code split into identifiers (paths such as `tokio::test` stay one
// token) and single punctuation characters.
fn tokens(code: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = code.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if is_identifier_character(c) {
            current.push(c);
            at += 1;
        } else if c == ':'
            && chars.get(at + 1) == Some(&':')
            && !current.is_empty()
            && chars
                .get(at + 2)
                .is_some_and(|&n| is_identifier_character(n))
        {
            current.push_str("::");
            at += 2;
        } else {
            if !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            if !c.is_whitespace() {
                tokens.push(c.to_string());
            }
            at += 1;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

// A brace scope the item pass is inside.
enum Scope {
    Module(String),
    Body,
    TestMacro,
}

// The index just past the `]` that closes the attribute whose `[` sits at `open`.
fn attribute_end(tokens: &[String], open: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.as_str() {
            "[" => depth += 1,
            "]" => {
                depth -= 1;
                if depth == 0 {
                    return index + 1;
                }
            }
            _ => {}
        }
    }
    tokens.len()
}

/// - Reads `source` as Rust text, with comments, strings and character literals blanked out.
pub fn functions(source: &str) -> Vec<Function> {
    let tokens = tokens(&code_only(source));
    let mut functions = Vec::new();
    let mut scopes: Vec<Scope> = Vec::new();
    let mut pending_module: Option<String> = None;
    let mut test_attribute = false;
    let mut at = 0;
    while at < tokens.len() {
        let token = tokens[at].as_str();
        let next = tokens.get(at + 1).map(String::as_str);
        match token {
            "#" => {
                let open = if next == Some("!") { at + 2 } else { at + 1 };
                if tokens.get(open).map(String::as_str) == Some("[") {
                    let path = tokens.get(open + 1).map(String::as_str).unwrap_or_default();
                    test_attribute |= TEST_ATTRIBUTE_PATHS.contains(&path);
                    at = attribute_end(&tokens, open);
                    continue;
                }
            }
            "fn" => {
                if let Some(name) = next.filter(|name| {
                    name.chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_')
                }) {
                    functions.push(Function {
                        name: name.to_string(),
                        modules: scopes
                            .iter()
                            .filter_map(|scope| match scope {
                                Scope::Module(name) => Some(name.clone()),
                                Scope::Body | Scope::TestMacro => None,
                            })
                            .collect(),
                        enclosing_bodies: scopes
                            .iter()
                            .filter(|scope| matches!(scope, Scope::Body))
                            .count(),
                        test_attribute,
                    });
                    test_attribute = false;
                    at += 2;
                    continue;
                }
            }
            "mod" => {
                pending_module = next.map(str::to_string);
                test_attribute = false;
            }
            path if TEST_MACRO_PATHS.contains(&path)
                && next == Some("!")
                && tokens.get(at + 2).map(String::as_str) == Some("{") =>
            {
                scopes.push(Scope::TestMacro);
                test_attribute = false;
                at += 3;
                continue;
            }
            "{" => {
                scopes.push(match pending_module.take() {
                    Some(name) => Scope::Module(name),
                    None => Scope::Body,
                });
                test_attribute = false;
            }
            "}" => {
                scopes.pop();
                test_attribute = false;
            }
            ";" => {
                pending_module = None;
                test_attribute = false;
            }
            keyword if ITEM_KEYWORDS.contains(&keyword) => test_attribute = false,
            _ => {}
        }
        at += 1;
    }
    functions
}

pub fn placement(source: &str, name: &str) -> TestPlacement {
    functions(source)
        .iter()
        .find(|function| function.name == name)
        .map(Function::placement)
        .unwrap_or(TestPlacement::Absent)
}

#[cfg(test)]
mod tests;
