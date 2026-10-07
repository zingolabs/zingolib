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

const WALLET_REPORT_RETURN: &str = "Result<String, ZingolibError>";

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

fn exported(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let path = attr.path();
        is_uniffi(path) && path.segments.len() == 2 && path.segments[1].ident == EXPORT
    })
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
        let plain_uniffi = attr.path().is_ident(UNIFFI);
        let export_with_arguments =
            exported(std::slice::from_ref(attr)) && matches!(attr.meta, Meta::List(_));
        if plain_uniffi || export_with_arguments {
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

fn wallet_report_line(mac: &ItemMacro) -> String {
    let input = mac.mac.tokens.to_string();
    let (signature, _) = input.split_once("=>").unwrap();
    let func: ItemFn =
        syn::parse_str(&format!("{signature} -> {WALLET_REPORT_RETURN} {{}}")).unwrap();
    function_line(&func)
}

fn generator_name(mac: &ItemMacro) -> Option<String> {
    let definition = mac.mac.path.is_ident(MACRO_RULES);
    let emits_uniffi = compact(&mac.mac.tokens).contains(&format!("{UNIFFI}::"));
    (definition && emits_uniffi).then(|| mac.ident.as_ref().unwrap().to_string())
}

fn macro_line(mac: &ItemMacro) -> Option<String> {
    let path = &mac.mac.path;
    if path.is_ident(WALLET_REPORT) {
        return Some(wallet_report_line(mac));
    }
    is_uniffi(path).then(|| format!("{}!({})", compact(path), compact(&mac.mac.tokens)))
}

fn collect(items: &[Item], surface: &mut Surface) {
    for item in items {
        match item {
            Item::Fn(func) if !cfg_test(&func.attrs) && exported(&func.attrs) => {
                surface.lines.insert(function_line(func));
            }
            Item::Impl(item) if !cfg_test(&item.attrs) && exported(&item.attrs) => {
                surface.lines.extend(impl_lines(item));
            }
            Item::Trait(item) if !cfg_test(&item.attrs) && exported(&item.attrs) => {
                surface.unrendered.insert(format!("trait {}", item.ident));
            }
            Item::Enum(item) if !cfg_test(&item.attrs) && derives_uniffi(&item.attrs) => {
                surface.lines.insert(enum_line(item));
            }
            Item::Struct(item) if !cfg_test(&item.attrs) && derives_uniffi(&item.attrs) => {
                surface.lines.insert(record_line(item));
            }
            Item::Macro(mac) if !cfg_test(&mac.attrs) => {
                surface.generators.extend(generator_name(mac));
                surface.lines.extend(macro_line(mac));
            }
            Item::Mod(module) if !cfg_test(&module.attrs) => {
                if let Some((_, items)) = &module.content {
                    collect(items, surface);
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

fn surface() -> Surface {
    let mut surface = Surface {
        lines: BTreeSet::new(),
        generators: BTreeSet::new(),
        unrendered: BTreeSet::new(),
    };
    for file in source_files(&Path::new(CRATE_DIR).join(SOURCE_DIR)) {
        let parsed = syn::parse_file(&fs::read_to_string(&file).unwrap()).unwrap();
        collect(&parsed.items, &mut surface);
    }
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
