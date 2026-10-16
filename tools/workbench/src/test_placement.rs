/// Where a test function sits in a source file, as nextest's naming of an integration test
/// sees it: only a top-level test keeps its bare name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestPlacement {
    TopLevel,
    Nested,
    Unattributed,
    Absent,
}

/// One `fn` item a Rust source defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: String,
    /// The enclosing modules, outermost first; empty at the top level of the file.
    pub modules: Vec<String>,
    /// The braces open around the item that are not modules: function bodies, impl blocks and
    /// other blocks, inside which the test harness collects nothing.
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
        unimplemented!("the placement of {} under {:?}", self.name, self.modules)
    }
}

/// - Reads `source` as Rust text, with comments, strings and character literals blanked out.
pub fn functions(source: &str) -> Vec<Function> {
    unimplemented!("the functions of {} bytes of source", source.len())
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
