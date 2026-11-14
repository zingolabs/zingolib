use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use crate::test_placement::{is_identifier_character, Function};

pub const NEXTEST_CONFIG: &str = ".config/nextest.toml";

pub const PROFILES: [&str; 2] = ["default", "ci"];

const DIRECTORIES_WITHOUT_SOURCES: [&str; 4] = [".git", "target", "build", "node_modules"];

const FILTER_KEY: &str = "filter";

// How a `test()` predicate matches a test's nextest name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestMatcher {
    Contains(String),
    Equals(String),
    Regex(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FiltersetTerm {
    Binary(String),
    Package(String),
    Test(TestMatcher),
}

// A nextest filterset expression over the predicates the ceilings use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filterset {
    Term(FiltersetTerm),
    Not(Box<Filterset>),
    And(Vec<Filterset>),
    Or(Vec<Filterset>),
}

impl Filterset {
    pub fn test_matchers(&self) -> Vec<&TestMatcher> {
        match self {
            Filterset::Term(FiltersetTerm::Test(matcher)) => vec![matcher],
            Filterset::Term(_) => vec![],
            Filterset::Not(inner) => inner.test_matchers(),
            Filterset::And(parts) | Filterset::Or(parts) => {
                parts.iter().flat_map(Filterset::test_matchers).collect()
            }
        }
    }
}

// One `[[profile.<name>.overrides]]` table: its filterset and every other key as TOML text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Override {
    pub filter: Filterset,
    pub settings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Or,
    And,
    Not,
    Open,
    Close,
    Predicate { name: String, argument: String },
}

fn tokens(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let character = chars[at];
        match character {
            whitespace if whitespace.is_whitespace() => at += 1,
            '|' | '&' | '(' | ')' => {
                tokens.push(match character {
                    '|' => Token::Or,
                    '&' => Token::And,
                    '(' => Token::Open,
                    _ => Token::Close,
                });
                at += 1;
            }
            start if start.is_alphabetic() || start == '_' => {
                let word_start = at;
                while chars.get(at).is_some_and(|c| is_identifier_character(*c)) {
                    at += 1;
                }
                let word: String = chars[word_start..at].iter().collect();
                if chars.get(at) == Some(&'(') {
                    let close = chars[at..]
                        .iter()
                        .position(|&c| c == ')')
                        .map(|offset| at + offset)
                        .ok_or_else(|| format!("{word}(: the predicate's argument is unclosed"))?;
                    let argument: String = chars[at + 1..close].iter().collect();
                    tokens.push(Token::Predicate {
                        name: word,
                        argument: argument.trim().to_string(),
                    });
                    at = close + 1;
                } else if word == "not" {
                    tokens.push(Token::Not);
                } else {
                    return Err(format!(
                        "{word}: a filterset term is a predicate over one argument"
                    ));
                }
            }
            other => return Err(format!("{other}: not a character of a filterset")),
        }
    }
    Ok(tokens)
}

fn matcher(argument: &str) -> TestMatcher {
    if let Some(exact) = argument.strip_prefix('=') {
        TestMatcher::Equals(exact.trim().to_string())
    } else if let Some(part) = argument.strip_prefix('~') {
        TestMatcher::Contains(part.trim().to_string())
    } else if let Some(pattern) = argument
        .strip_prefix('/')
        .and_then(|rest| rest.strip_suffix('/'))
    {
        TestMatcher::Regex(pattern.to_string())
    } else {
        TestMatcher::Contains(argument.to_string())
    }
}

fn term(name: &str, argument: &str) -> Result<FiltersetTerm, String> {
    match name {
        "binary" => Ok(FiltersetTerm::Binary(argument.to_string())),
        "package" => Ok(FiltersetTerm::Package(argument.to_string())),
        "test" => Ok(FiltersetTerm::Test(matcher(argument))),
        other => Err(format!(
            "{other}({argument}): the ceilings use no {other} predicate"
        )),
    }
}

// The two binary operator levels of a filterset, loosest first.
#[derive(Clone, Copy)]
enum Precedence {
    Or,
    And,
}

struct Parser {
    tokens: Vec<Token>,
    at: usize,
}

impl Parser {
    fn next_is(&self, token: &Token) -> bool {
        self.tokens.get(self.at) == Some(token)
    }

    fn operands(&mut self, precedence: Precedence) -> Result<Filterset, String> {
        let (operator, join): (Token, fn(Vec<Filterset>) -> Filterset) = match precedence {
            Precedence::Or => (Token::Or, Filterset::Or),
            Precedence::And => (Token::And, Filterset::And),
        };
        let operand = |parser: &mut Parser| match precedence {
            Precedence::Or => parser.operands(Precedence::And),
            Precedence::And => parser.unary(),
        };
        let mut parts = vec![operand(self)?];
        while self.next_is(&operator) {
            self.at += 1;
            parts.push(operand(self)?);
        }
        Ok(match parts.len() {
            1 => parts.remove(0),
            _ => join(parts),
        })
    }

    fn unary(&mut self) -> Result<Filterset, String> {
        let token = self
            .tokens
            .get(self.at)
            .cloned()
            .ok_or_else(|| "the filterset ends where a term should stand".to_string())?;
        self.at += 1;
        match token {
            Token::Not => Ok(Filterset::Not(Box::new(self.unary()?))),
            Token::Open => {
                let inner = self.operands(Precedence::Or)?;
                if !self.next_is(&Token::Close) {
                    return Err("a parenthesis is unclosed".to_string());
                }
                self.at += 1;
                Ok(inner)
            }
            Token::Predicate { name, argument } => Ok(Filterset::Term(term(&name, &argument)?)),
            Token::Close => Err("a parenthesis closes nothing".to_string()),
            Token::Or | Token::And => Err("an operator has no left operand".to_string()),
        }
    }
}

pub fn filterset(text: &str) -> Result<Filterset, String> {
    let mut parser = Parser {
        tokens: tokens(text)?,
        at: 0,
    };
    let filterset = parser.operands(Precedence::Or)?;
    if parser.at < parser.tokens.len() {
        return Err(format!("{text}: a term follows the filterset"));
    }
    Ok(filterset)
}

pub fn overrides(config: &str, profile: &str) -> Result<Vec<Override>, String> {
    let document: DocumentMut = config
        .parse()
        .map_err(|error| format!("{NEXTEST_CONFIG}: {error}"))?;
    let overrides = document
        .get("profile")
        .and_then(|profiles| profiles.get(profile))
        .and_then(|profile| profile.get("overrides"))
        .and_then(Item::as_array_of_tables)
        .ok_or_else(|| format!("profile.{profile}.overrides is not an array of tables"))?;
    overrides
        .iter()
        .map(|table| {
            let filter = table
                .get(FILTER_KEY)
                .and_then(Item::as_str)
                .ok_or_else(|| format!("an override of profile.{profile} has no string filter"))?;
            let settings = table
                .iter()
                .filter(|(key, _)| *key != FILTER_KEY)
                .map(|(key, item)| (key.to_string(), item.to_string().trim().to_string()))
                .collect();
            Ok(Override {
                filter: filterset(filter)?,
                settings,
            })
        })
        .collect()
}

impl TestMatcher {
    pub fn matches(&self, nextest_name: &str) -> Result<bool, String> {
        match self {
            TestMatcher::Contains(part) => Ok(nextest_name.contains(part)),
            TestMatcher::Equals(exact) => Ok(nextest_name == exact),
            TestMatcher::Regex(pattern) => Err(format!(
                "test(/{pattern}/): nextest accepts a regex matcher, and this gate cannot judge one"
            )),
        }
    }
}

/// - Returns an error for a matcher the ceilings cannot evaluate, such as a regex.
pub fn names_a_defined_test(matcher: &TestMatcher, functions: &[Function]) -> Result<bool, String> {
    for function in functions
        .iter()
        .filter(|function| function.is_harness_test())
    {
        if matcher.matches(&function.nextest_name())? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// - Reads every directory under `root` except the ones that hold no sources.
pub fn rust_sources(root: &Path) -> Result<Vec<PathBuf>, Vec<String>> {
    Ok(crate::files_under(root, &DIRECTORIES_WITHOUT_SOURCES)?
        .into_iter()
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect())
}

#[cfg(test)]
mod tests;
