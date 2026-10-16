const TEST_ATTRIBUTE_PATHS: [&str; 2] = ["test", "tokio::test"];

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestPlacement {
    TopLevel,
    Nested,
    Unattributed,
    Absent,
}

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

enum Region {
    Code,
    LineComment,
    BlockComment(usize),
    Str,
    RawStr(usize),
    Char,
}

fn is_identifier_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

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

fn opens_character_literal(chars: &[char], at: usize) -> bool {
    matches!(
        (chars.get(at + 1), chars.get(at + 2)),
        (Some('\\'), _) | (Some(_), Some('\''))
    )
}

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

enum Scope {
    Module(String),
    Body,
}

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
                                Scope::Body => None,
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
mod tests {
    use super::*;

    const TOP_LEVEL_TEST: &str =
        "use x;\n\nfn helper() {\n}\n\n/// doc { not code }\n#[tokio::test]\n#[ignore]\n\
         async fn lite() {\n}\n";

    fn names(source: &str) -> Vec<String> {
        functions(source)
            .into_iter()
            .map(|function| function.name)
            .collect()
    }

    #[test]
    fn every_fn_item_is_read_whatever_its_modifiers_and_generics() {
        let source = "fn plain() {}\npub fn public() {}\npub(crate) async fn crate_async() {}\n\
                      pub(super) const fn constant<T>(value: T) {}\nunsafe fn hazardous() {}\n\
                      pub(in crate::a) fn scoped() {}\n";
        assert_eq!(
            names(source),
            [
                "plain",
                "public",
                "crate_async",
                "constant",
                "hazardous",
                "scoped"
            ]
        );
    }

    #[test]
    fn words_that_merely_contain_fn_and_closures_are_not_functions() {
        let source = "let defn = 1;\nlet fn_count = 2;\nlet closure = |x| x;\n\
                      struct Defn;\nfn real() {}\n";
        assert_eq!(names(source), ["real"]);
    }

    #[test]
    fn strings_characters_and_comments_hide_their_braces_and_their_fns() {
        let source = "fn outer() {\n    let a = \"fn fake() {\";\n    let b = '{';\n\
                      let c = r#\"fn raw() { \"# ;\n    foo(\"http://x\", || {});\n\
                      /* fn commented() { */\n    // fn lined() {\n}\n\
                      #[test]\nfn after() {}\n";
        let functions = functions(source);
        assert_eq!(
            functions
                .iter()
                .map(|function| function.name.as_str())
                .collect::<Vec<_>>(),
            ["outer", "after"]
        );
        assert_eq!(functions[1].enclosing_bodies, 0);
        assert_eq!(functions[1].placement(), TestPlacement::TopLevel);
    }

    #[test]
    fn a_test_attribute_spanning_lines_still_attributes_the_function() {
        let source = "#[tokio::test(\n    flavor = \"multi_thread\",\n    worker_threads = 2\n)]\n\
                      async fn lite() {}\n#[cfg_attr(\n    feature = \"x\",\n    ignore\n)]\n\
                      fn plain() {}\n";
        let functions = functions(source);
        assert!(functions[0].test_attribute, "{functions:?}");
        assert!(!functions[1].test_attribute, "{functions:?}");
    }

    #[test]
    fn a_nested_test_carries_its_module_path_as_nextest_names_it() {
        let source = "mod outer {\n    pub(crate) mod inner {\n        #[test]\n        fn deep() {}\n    }\n\
                      }\n#[cfg(test)]\nmod tests {\n    #[tokio::test]\n    async fn shallow() {}\n}\n";
        let functions = functions(source);
        assert_eq!(functions[0].nextest_name(), "outer::inner::deep");
        assert_eq!(functions[0].placement(), TestPlacement::Nested);
        assert!(functions[0].is_harness_test());
        assert_eq!(functions[1].nextest_name(), "tests::shallow");
        assert_eq!(functions[1].modules, ["tests"]);
    }

    #[test]
    fn a_function_inside_a_body_or_an_impl_is_no_harness_test() {
        let source = "fn outer() {\n    #[test]\n    fn inner() {}\n}\n\
                      impl Thing {\n    #[test]\n    fn method(&self) {}\n}\n";
        let functions = functions(source);
        assert_eq!(functions[1].name, "inner");
        assert_eq!(functions[1].enclosing_bodies, 1);
        assert!(!functions[1].is_harness_test());
        assert_eq!(functions[1].placement(), TestPlacement::Nested);
        assert_eq!(functions[2].name, "method");
        assert!(!functions[2].is_harness_test());
    }

    #[test]
    fn a_test_attribute_above_a_top_level_function_places_it_at_the_top_level() {
        assert_eq!(placement(TOP_LEVEL_TEST, "lite"), TestPlacement::TopLevel);
        assert_eq!(
            placement("#[test]\nfn lite() {}\n", "lite"),
            TestPlacement::TopLevel
        );
        assert_eq!(
            placement(
                "#[tokio::test(flavor = \"multi_thread\")]\npub async fn lite() {}\n",
                "lite"
            ),
            TestPlacement::TopLevel
        );
    }

    #[test]
    fn a_function_without_a_test_attribute_is_unattributed() {
        assert_eq!(
            placement("async fn lite() {}\n", "lite"),
            TestPlacement::Unattributed
        );
        assert_eq!(
            placement(
                "#[tokio::test]\nasync fn other() {}\n\nasync fn lite() {}\n",
                "lite"
            ),
            TestPlacement::Unattributed
        );
        assert_eq!(
            placement("#[ignore]\nasync fn lite() {}\n", "lite"),
            TestPlacement::Unattributed
        );
    }

    #[test]
    fn a_function_inside_a_module_or_another_body_is_nested() {
        for wrapper in [
            "mod boundary {",
            "pub mod boundary {",
            "pub(crate) mod boundary {",
            "fn outer() {",
        ] {
            let source = format!("{wrapper}\n    #[tokio::test]\n    async fn lite() {{}}\n}}\n");
            assert_eq!(
                placement(&source, "lite"),
                TestPlacement::Nested,
                "{wrapper}"
            );
        }
    }

    #[test]
    fn a_missing_function_is_absent() {
        assert_eq!(
            placement(
                "#[tokio::test]\nasync fn lite_other() {}\n// fn lite()\n",
                "lite"
            ),
            TestPlacement::Absent
        );
    }
}
