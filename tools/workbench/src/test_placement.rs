const TEST_ATTRIBUTES: [&str; 2] = ["#[test]", "#[tokio::test"];

// Where a test function sits in a source file, as nextest's naming of an
// integration test sees it: only a top-level test keeps its bare name.
#[derive(Debug, PartialEq, Eq)]
pub enum TestPlacement {
    TopLevel,
    Nested,
    Unattributed,
    Absent,
}

fn is_test_attribute(line: &str) -> bool {
    TEST_ATTRIBUTES
        .iter()
        .any(|attribute| line.starts_with(attribute))
}

fn without_line_comment(line: &str) -> &str {
    line.split("//").next().unwrap_or_default()
}

pub fn placement(source: &str, name: &str) -> TestPlacement {
    let signatures = [format!("async fn {name}("), format!("fn {name}(")];
    let mut depth: i64 = 0;
    let mut attributed = false;
    for line in source.lines() {
        let code = without_line_comment(line).trim();
        if signatures
            .iter()
            .any(|signature| code.starts_with(signature))
        {
            return match (depth, attributed) {
                (0, true) => TestPlacement::TopLevel,
                (0, false) => TestPlacement::Unattributed,
                (_, _) => TestPlacement::Nested,
            };
        }
        attributed = match code {
            "" => attributed,
            attribute if attribute.starts_with("#[") => attributed || is_test_attribute(attribute),
            _ => false,
        };
        depth += code.matches('{').count() as i64 - code.matches('}').count() as i64;
    }
    TestPlacement::Absent
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP_LEVEL_TEST: &str =
        "use x;\n\nfn helper() {\n}\n\n/// doc { not code }\n#[tokio::test]\n#[ignore]\n\
         async fn lite() {\n}\n";

    #[test]
    fn a_test_attribute_above_a_top_level_function_places_it_at_the_top_level() {
        assert_eq!(placement(TOP_LEVEL_TEST, "lite"), TestPlacement::TopLevel);
        assert_eq!(
            placement("#[test]\nfn lite() {}\n", "lite"),
            TestPlacement::TopLevel
        );
        assert_eq!(
            placement(
                "#[tokio::test(flavor = \"multi_thread\")]\nasync fn lite() {}\n",
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
