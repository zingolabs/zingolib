#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, FnArg, Item, ItemEnum, ItemFn, ItemImpl, ItemMacro, ItemStruct, Meta,
    Path as SynPath, ReturnType, Signature, Token,
};

const CRATE_DIR: &str = env!("CARGO_MANIFEST_DIR");

const SOURCE_DIR: &str = "src";

const CONTRACT_FILE: &str = "src/exported_surface.txt";

const BINDGEN_CONFIG_FILE: &str = "uniffi.toml";

const LOCKFILE: &str = "../../Cargo.lock";

const WALLET_REPORT: &str = "wallet_report";

const MACRO_FRAGMENT: char = '$';

const TEMPLATE_IDENT: &str = "template";

const REPETITION_MARKERS: [char; 3] = ['?', '*', '+'];

const RULE_ARROW: &str = "=>";

const RULE_END: char = ';';

const CONFIG_PREFIX: &str = "uniffi.toml: ";

const LOCKED_PREFIX: &str = "lockfile: uniffi ";

const PACKAGE_PREFIX: &str = "package ";

const LOCKED_NAME_LINE: &str = "name = \"uniffi\"";

const LOCKED_VERSION_PREFIX: &str = "version = ";

const UNIFFI: &str = "uniffi";

const EXPORT: &str = "export";

const MACRO_RULES: &str = "macro_rules";

const CFG: &str = "cfg";

const DERIVE: &str = "derive";

const TEST: &str = "test";

const EXPANDED_MACROS: [&str; 1] = [WALLET_REPORT];

const SPACING: [(&str, &str); 12] = [
    (" :: ", "::"),
    (" < ", "<"),
    (" > ", ">"),
    (" >", ">"),
    (" ,", ","),
    (" (", "("),
    ("( ", "("),
    (" )", ")"),
    (" !", "!"),
    ("# [", "#["),
    ("[ ", "["),
    (" ]", "]"),
];

struct Surface {
    lines: BTreeSet<String>,
    generators: BTreeSet<String>,
    unrendered: BTreeSet<String>,
}

struct WalletReportTemplate {
    markers: String,
    ret: String,
}

fn compact<T: ToTokens>(tokens: &T) -> String {
    SPACING
        .iter()
        .fold(tokens.to_token_stream().to_string(), |text, (from, to)| {
            text.replace(from, to)
        })
}

fn is_uniffi(path: &SynPath) -> bool {
    path.segments
        .first()
        .is_some_and(|segment| segment.ident == UNIFFI)
}

fn is_uniffi_attribute(attr: &Attribute) -> bool {
    let path = attr.path();
    path.is_ident(UNIFFI) || (is_uniffi(path) && path.segments.len() == 2)
}

fn is_plain_export(attr: &Attribute) -> bool {
    let path = attr.path();
    is_uniffi(path)
        && path.segments.len() == 2
        && path.segments[1].ident == EXPORT
        && matches!(attr.meta, Meta::Path(_))
}

fn cfg_test(attrs: &[Attribute]) -> bool {
    attrs
        .iter()
        .any(|attr| attr.path().is_ident(CFG) && compact(&attr.meta) == format!("{CFG}({TEST})"))
}

fn uniffi_derives(attr: &Attribute) -> Vec<String> {
    if !attr.path().is_ident(DERIVE) {
        return Vec::new();
    }
    attr.parse_args_with(Punctuated::<SynPath, Token![,]>::parse_terminated)
        .unwrap()
        .iter()
        .filter(|path| is_uniffi(path))
        .map(compact)
        .collect()
}

fn markers(attrs: &[Attribute]) -> String {
    let mut markers = Vec::new();
    for attr in attrs {
        let derives = uniffi_derives(attr);
        if !derives.is_empty() {
            markers.push(format!("#[{DERIVE}({})]", derives.join(", ")));
        }
        if is_uniffi_attribute(attr) && !is_plain_export(attr) {
            markers.push(format!("#[{}]", compact(&attr.meta)));
        }
    }
    markers
        .into_iter()
        .map(|marker| format!("{marker} "))
        .collect()
}

fn derives_uniffi(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| !uniffi_derives(attr).is_empty())
}

fn record_line(item: &ItemStruct) -> String {
    let fields = item
        .fields
        .iter()
        .map(|field| {
            format!(
                "{}{}: {}",
                markers(&field.attrs),
                field.ident.as_ref().unwrap(),
                compact(&field.ty)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}{} {{ {fields} }}", markers(&item.attrs), item.ident)
}

fn enum_line(item: &ItemEnum) -> String {
    let variants = item
        .variants
        .iter()
        .map(|variant| format!("{}{}", markers(&variant.attrs), variant.ident))
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{}{} = {variants}", markers(&item.attrs), item.ident)
}

fn signature_line(sig: &Signature, attrs: &[Attribute]) -> String {
    let params = sig
        .inputs
        .iter()
        .map(|arg| match arg {
            FnArg::Typed(typed) => format!(
                "{}{}: {}",
                markers(&typed.attrs),
                typed.pat.to_token_stream(),
                compact(&typed.ty)
            ),
            FnArg::Receiver(receiver) => receiver.to_token_stream().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret = match &sig.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => compact(ty),
    };
    format!("{}{}({params}) -> {ret}", markers(attrs), sig.ident)
}

fn function_line(func: &ItemFn) -> String {
    signature_line(&func.sig, &func.attrs)
}

fn impl_lines(item: &ItemImpl) -> Vec<String> {
    let owner = format!("{}impl {}", markers(&item.attrs), compact(&item.self_ty));
    item.items
        .iter()
        .filter_map(|member| match member {
            syn::ImplItem::Fn(method) => Some(format!(
                "{owner}::{}",
                signature_line(&method.sig, &method.attrs)
            )),
            _ => None,
        })
        .collect()
}

fn is_wallet_report_definition(mac: &ItemMacro) -> bool {
    mac.mac.path.is_ident(MACRO_RULES)
        && mac
            .ident
            .as_ref()
            .is_some_and(|ident| ident == WALLET_REPORT)
}

fn rule_expansion(rule: &str) -> &str {
    let mut depth = 0;
    let matcher_end = rule
        .char_indices()
        .find_map(|(index, c)| {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
            (c == ')' && depth == 0).then_some(index + 1)
        })
        .unwrap();
    let after_arrow = rule[matcher_end..]
        .trim_start()
        .strip_prefix(RULE_ARROW)
        .unwrap()
        .trim()
        .trim_end_matches(RULE_END)
        .trim_end();
    after_arrow
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap()
}

fn rendered_template(expansion: &str) -> String {
    let chars: Vec<char> = expansion.chars().collect();
    let skip_space = |mut i: usize| {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        i
    };
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != MACRO_FRAGMENT {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        i = skip_space(i + 1);
        if chars.get(i) == Some(&'(') {
            let mut depth = 0;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                i += 1;
                if depth == 0 {
                    break;
                }
            }
            let after_group = skip_space(i);
            let is_marker = |index: usize| {
                chars
                    .get(index)
                    .is_some_and(|c| REPETITION_MARKERS.contains(c))
            };
            if is_marker(after_group) {
                i = after_group + 1;
            } else {
                let after_separator = skip_space(after_group + 1);
                if is_marker(after_separator) {
                    i = after_separator + 1;
                }
            }
        } else {
            while chars
                .get(i)
                .is_some_and(|c| c.is_alphanumeric() || *c == '_')
            {
                i += 1;
            }
            out.push_str(TEMPLATE_IDENT);
        }
    }
    out
}

fn wallet_report_template(mac: &ItemMacro) -> WalletReportTemplate {
    let rule = mac.mac.tokens.to_string();
    let rendered = rendered_template(rule_expansion(&rule));
    assert!(
        !rendered.contains(MACRO_FRAGMENT),
        "the {WALLET_REPORT} template holds a fragment this test cannot render: {rendered}"
    );
    let func: ItemFn = syn::parse_str(&rendered).unwrap();
    let ret = match &func.sig.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => compact(ty),
    };
    WalletReportTemplate {
        markers: markers(&func.attrs),
        ret,
    }
}

fn wallet_report_line(mac: &ItemMacro, template: Option<&WalletReportTemplate>) -> String {
    let template = template.unwrap_or_else(|| {
        panic!("a {WALLET_REPORT}! invocation precedes any macro_rules! {WALLET_REPORT} definition")
    });
    let input = mac.mac.tokens.to_string();
    let (signature, _) = input.split_once(RULE_ARROW).unwrap();
    let func: ItemFn = syn::parse_str(&format!("{signature} -> {} {{}}", template.ret)).unwrap();
    format!("{}{}", template.markers, function_line(&func))
}

fn generator_name(mac: &ItemMacro) -> Option<String> {
    let definition = mac.mac.path.is_ident(MACRO_RULES);
    let emits_uniffi = compact(&mac.mac.tokens).contains(&format!("{UNIFFI}::"));
    (definition && emits_uniffi).then(|| mac.ident.as_ref().unwrap().to_string())
}

fn macro_line(mac: &ItemMacro, template: Option<&WalletReportTemplate>) -> Option<String> {
    let path = &mac.mac.path;
    if path.is_ident(WALLET_REPORT) {
        return Some(wallet_report_line(mac, template));
    }
    is_uniffi(path).then(|| format!("{}!({})", compact(path), compact(&mac.mac.tokens)))
}

fn find_template(items: &[Item]) -> Option<WalletReportTemplate> {
    items.iter().find_map(|item| match item {
        Item::Macro(mac) if is_wallet_report_definition(mac) => Some(wallet_report_template(mac)),
        Item::Mod(module) => module
            .content
            .as_ref()
            .and_then(|(_, items)| find_template(items)),
        _ => None,
    })
}

fn collect(items: &[Item], surface: &mut Surface, template: Option<&WalletReportTemplate>) {
    for item in items {
        match item {
            Item::Fn(func)
                if !cfg_test(&func.attrs) && func.attrs.iter().any(is_uniffi_attribute) =>
            {
                surface.lines.insert(function_line(func));
            }
            Item::Impl(item)
                if !cfg_test(&item.attrs) && item.attrs.iter().any(is_uniffi_attribute) =>
            {
                surface.lines.extend(impl_lines(item));
            }
            Item::Trait(item)
                if !cfg_test(&item.attrs) && item.attrs.iter().any(is_uniffi_attribute) =>
            {
                surface.unrendered.insert(format!("trait {}", item.ident));
            }
            Item::Enum(item)
                if !cfg_test(&item.attrs)
                    && (derives_uniffi(&item.attrs)
                        || item.attrs.iter().any(is_uniffi_attribute)) =>
            {
                surface.lines.insert(enum_line(item));
            }
            Item::Struct(item)
                if !cfg_test(&item.attrs)
                    && (derives_uniffi(&item.attrs)
                        || item.attrs.iter().any(is_uniffi_attribute)) =>
            {
                surface.lines.insert(record_line(item));
            }
            Item::Macro(mac) if !cfg_test(&mac.attrs) => {
                surface.generators.extend(generator_name(mac));
                surface.lines.extend(macro_line(mac, template));
            }
            Item::Mod(module) if !cfg_test(&module.attrs) => {
                if let Some((_, items)) = &module.content {
                    collect(items, surface, template);
                }
            }
            _ => {}
        }
    }
}

fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(source_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn read(relative: &str) -> String {
    fs::read_to_string(Path::new(CRATE_DIR).join(relative)).unwrap()
}

fn locked_uniffi_versions() -> Vec<String> {
    let lockfile = read(LOCKFILE);
    let mut lines = lockfile.lines();
    let mut versions = Vec::new();
    while let Some(line) = lines.next() {
        if line == LOCKED_NAME_LINE {
            let version = lines
                .next()
                .and_then(|line| line.strip_prefix(LOCKED_VERSION_PREFIX))
                .unwrap();
            versions.push(format!("{LOCKED_PREFIX}{version}"));
        }
    }
    versions
}

fn surface_of(sources: &[String]) -> Surface {
    let parsed: Vec<syn::File> = sources
        .iter()
        .map(|source| syn::parse_file(source).unwrap())
        .collect();
    let template = parsed.iter().find_map(|file| find_template(&file.items));
    let mut surface = Surface {
        lines: BTreeSet::new(),
        generators: BTreeSet::new(),
        unrendered: BTreeSet::new(),
    };
    for file in &parsed {
        collect(&file.items, &mut surface, template.as_ref());
    }
    surface
}

fn surface() -> Surface {
    let sources: Vec<String> = source_files(&Path::new(CRATE_DIR).join(SOURCE_DIR))
        .iter()
        .map(|file| fs::read_to_string(file).unwrap())
        .collect();
    let mut surface = surface_of(&sources);
    surface.lines.extend(
        read(BINDGEN_CONFIG_FILE)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| format!("{CONFIG_PREFIX}{line}")),
    );
    surface.lines.extend(locked_uniffi_versions());
    surface
        .lines
        .insert(format!("{PACKAGE_PREFIX}{}", env!("CARGO_PKG_NAME")));
    surface
}

#[test]
fn the_library_exports_the_committed_surface() {
    let exported = surface().lines;
    let exported: BTreeSet<&str> = exported.iter().map(String::as_str).collect();
    let contract = read(CONTRACT_FILE);
    let committed: BTreeSet<&str> = contract.lines().collect();
    let missing: Vec<_> = committed.difference(&exported).collect();
    let unlisted: Vec<_> = exported.difference(&committed).collect();
    assert!(
        missing.is_empty() && unlisted.is_empty(),
        "committed but not exported:\n{missing:#?}\nexported but not committed:\n{unlisted:#?}"
    );
}

#[test]
fn the_wallet_report_body_governs_every_line_its_invocations_render() {
    let source = r#"
        macro_rules! wallet_report {
            (pub fn $export:ident($($input:ident: String)?) => $access:ident($state:path)) => {
                #[uniffi::export(name = "renamed")]
                pub fn $export($($input: String)?) -> Result<Vec<u8>, ZingolibError> {
                    $access($($input,)? $state)
                }
            };
        }
        wallet_report!(pub fn get_seed() => report_wallet(Seed::of));
        wallet_report!(pub fn remove_transaction(txid: String) => report_wallet_mut_with(Removal::of));
    "#;
    let lines = surface_of(&[source.to_string()]).lines;
    let expected: BTreeSet<String> = [
        "#[uniffi::export(name = \"renamed\")] get_seed() -> Result<Vec<u8>, ZingolibError>",
        "#[uniffi::export(name = \"renamed\")] remove_transaction(txid: String) -> Result<Vec<u8>, ZingolibError>",
    ]
    .map(ToString::to_string)
    .into();
    assert_eq!(lines, expected);
}

#[test]
fn every_uniffi_attribute_but_a_plain_export_marks_its_item() {
    let source = r#"
        #[derive(uniffi::Object)]
        pub struct Engine { state: u32 }
        #[uniffi::export]
        impl Engine {
            #[uniffi::constructor]
            pub fn new() -> Arc<Self> { todo!() }
            #[uniffi::method(name = "renamed")]
            pub fn report(&self) -> String { todo!() }
            pub fn plain(&self) -> u32 { todo!() }
        }
        #[uniffi::export(callback_interface)]
        pub trait Listener { fn on_event(&self); }
        #[uniffi::remote(Record)]
        pub struct Remote { pub id: u32 }
        #[uniffi::remote(Enum)]
        pub enum Kind { A, B }
        #[uniffi::export]
        pub fn free(#[uniffi(default = 1)] count: u32) -> u32 { count }
    "#;
    let surface = surface_of(&[source.to_string()]);
    let expected: BTreeSet<String> = [
        "#[derive(uniffi::Object)] Engine { state: u32 }",
        "impl Engine::#[uniffi::constructor] new() -> Arc<Self>",
        "impl Engine::#[uniffi::method(name = \"renamed\")] report(& self) -> String",
        "impl Engine::plain(& self) -> u32",
        "#[uniffi::remote(Record)] Remote { id: u32 }",
        "#[uniffi::remote(Enum)] Kind = A | B",
        "free(#[uniffi(default = 1)] count: u32) -> u32",
    ]
    .map(ToString::to_string)
    .into();
    assert_eq!(surface.lines, expected);
    assert_eq!(
        surface.unrendered,
        BTreeSet::from(["trait Listener".to_string()])
    );
}

#[test]
fn every_macro_that_emits_uniffi_items_is_expanded_here() {
    let generators = surface().generators;
    let expanded: BTreeSet<String> = EXPANDED_MACROS.iter().map(ToString::to_string).collect();
    assert_eq!(
        generators, expanded,
        "a macro_rules! body mentions uniffi; teach this test to expand its invocations"
    );
}

#[test]
fn no_exported_trait_waits_for_a_renderer() {
    let unrendered = surface().unrendered;
    assert!(
        unrendered.is_empty(),
        "an exported trait has no renderer here: {unrendered:?}"
    );
}

const WORKFLOW_FILE: &str = "../../.github/workflows/ci-pr.yaml";

const WORKFLOW_PATHS_KEY: &str = "paths:";

const WORKFLOW_PATH_ITEM: &str = "- \"";

const WORKFLOW_PATHS_REQUIRED: [&str; 4] = [
    "**/*.rs",
    "**/Cargo.lock",
    "**/uniffi.toml",
    "zingo-ffi/lib/src/exported_surface.txt",
];

const RETIRED_SPELLINGS: [&str; 3] = ["chainhint", "minconfirmations", "performancetype"];

const GENERIC_TYPE_NAMES: [&str; 8] = [
    "Client",
    "Config",
    "Connection",
    "Error",
    "Result",
    "Session",
    "Settings",
    "Status",
];

const UDL_LABELLED_SIGNATURES: [&str; 2] = [
    "get_latest_block_server(serveruri: String) -> Result<String, ZingolibError>",
    "change_server(serveruri: String) -> Result<String, ZingolibError>",
];

fn workflow_paths() -> BTreeSet<String> {
    let workflow = read(WORKFLOW_FILE);
    let mut paths = BTreeSet::new();
    let mut in_paths = false;
    for line in workflow.lines() {
        let trimmed = line.trim();
        if trimmed == WORKFLOW_PATHS_KEY {
            in_paths = true;
            continue;
        }
        if in_paths {
            match trimmed
                .strip_prefix(WORKFLOW_PATH_ITEM)
                .and_then(|rest| rest.strip_suffix('"'))
            {
                Some(path) => {
                    paths.insert(path.to_string());
                }
                None => in_paths = false,
            }
        }
    }
    paths
}

fn type_name(line: &str) -> Option<&str> {
    let derived = line.contains(&format!("#[{DERIVE}({UNIFFI}::"));
    derived.then(|| {
        let after_markers = line.rsplit_once("] ").map_or(line, |(_, rest)| rest);
        after_markers.split_whitespace().next().unwrap()
    })
}

#[test]
fn the_pull_request_filter_names_every_input_of_this_test() {
    let paths = workflow_paths();
    let missing: Vec<_> = WORKFLOW_PATHS_REQUIRED
        .iter()
        .filter(|path| !paths.contains(**path))
        .collect();
    assert!(missing.is_empty(), "ci-pr.yaml paths lack {missing:?}");
}

#[test]
fn no_udl_spelling_survives_in_the_sources() {
    let mut found = Vec::new();
    for file in source_files(&Path::new(CRATE_DIR).join(SOURCE_DIR)) {
        let text = fs::read_to_string(&file).unwrap();
        for (index, line) in text.lines().enumerate() {
            for spelling in RETIRED_SPELLINGS {
                if line.contains(spelling) {
                    found.push(format!("{}:{}: {spelling}", file.display(), index + 1));
                }
            }
        }
    }
    assert!(found.is_empty(), "retired UDL spellings:\n{found:#?}");
}

#[test]
fn every_exported_type_name_carries_a_domain() {
    let lines = surface().lines;
    let generic: Vec<_> = lines
        .iter()
        .filter_map(|line| type_name(line))
        .filter(|name| GENERIC_TYPE_NAMES.contains(name))
        .collect();
    assert!(
        generic.is_empty(),
        "these names collide with a consumer's own types: {generic:?}"
    );
}

#[test]
fn the_server_argument_keeps_its_udl_label() {
    let lines = surface().lines;
    let missing: Vec<_> = UDL_LABELLED_SIGNATURES
        .iter()
        .filter(|signature| !lines.contains(**signature))
        .collect();
    assert!(
        missing.is_empty(),
        "the Swift label changed for {missing:#?}"
    );
}
