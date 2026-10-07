#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use quote::ToTokens;
use syn::{Attribute, Item, ItemEnum, ItemFn, ItemMacro, ReturnType, Type};

const SOURCE: &str = include_str!("../src/lib.rs");

const CONTRACT: &str = include_str!("../src/exported_surface.txt");

const WALLET_REPORT_RETURN: &str = "Result<String, ZingolibError>";

fn tokens(ty: &Type) -> String {
    ty.to_token_stream()
        .to_string()
        .replace(" < ", "<")
        .replace(" > ", ">")
        .replace(" >", ">")
        .replace(" ,", ",")
        .replace(" (", "(")
        .replace(" )", ")")
}

fn exported(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().segments.len() == 2
            && attr.path().segments[0].ident == "uniffi"
            && attr.path().segments[1].ident == "export"
    })
}

fn derives_uniffi_error(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("derive")
            && attr
                .to_token_stream()
                .to_string()
                .contains("uniffi :: Error")
    })
}

fn function_line(func: &ItemFn) -> String {
    let params = func
        .sig
        .inputs
        .iter()
        .map(|arg| match arg {
            syn::FnArg::Typed(typed) => {
                format!("{}: {}", typed.pat.to_token_stream(), tokens(&typed.ty))
            }
            syn::FnArg::Receiver(receiver) => receiver.to_token_stream().to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    let ret = match &func.sig.output {
        ReturnType::Default => "()".to_string(),
        ReturnType::Type(_, ty) => tokens(ty),
    };
    format!("{}({params}) -> {ret}", func.sig.ident)
}

fn wallet_report_line(mac: &ItemMacro) -> Option<String> {
    mac.mac.path.is_ident("wallet_report").then(|| {
        let input = mac.mac.tokens.to_string();
        let (signature, _) = input.split_once("=>").unwrap();
        let func: ItemFn =
            syn::parse_str(&format!("{signature} -> {WALLET_REPORT_RETURN} {{}}")).unwrap();
        function_line(&func)
    })
}

fn enum_line(item: &ItemEnum) -> String {
    let variants = item
        .variants
        .iter()
        .map(|variant| variant.ident.to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    format!("{} = {variants}", item.ident)
}

#[test]
fn the_library_exports_the_committed_surface() {
    let file = syn::parse_file(SOURCE).unwrap();
    let exported: BTreeSet<String> = file
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Fn(func) if exported(&func.attrs) => Some(function_line(func)),
            Item::Enum(item) if derives_uniffi_error(&item.attrs) => Some(enum_line(item)),
            Item::Macro(mac) => wallet_report_line(mac),
            _ => None,
        })
        .collect();
    let exported: BTreeSet<&str> = exported.iter().map(String::as_str).collect();
    let committed: BTreeSet<&str> = CONTRACT.lines().collect();
    let missing: Vec<_> = committed.difference(&exported).collect();
    let unlisted: Vec<_> = exported.difference(&committed).collect();
    assert!(
        missing.is_empty() && unlisted.is_empty(),
        "committed but not exported:\n{missing:#?}\nexported but not committed:\n{unlisted:#?}"
    );
}
