//! The `strict-contracts` feature is on in every test build, and every type in
//! this crate that derives `Deserialize` honours it.
//!
//! The attribute is added by hand to each type, so a type that forgets it would
//! decode leniently in the contract tests and let an unknown server field through
//! unnoticed. The check reads the crate's own source with a Rust parser, so it
//! finds types in nested modules and across multi-line attributes, and it cannot
//! be satisfied by a comment.

use std::fs;
use std::path::{Path, PathBuf};

use district_model::NativeRevokeResponse;
use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{Attribute, Ident, ItemEnum, ItemImpl, ItemStruct, Meta, Token};

/// The attribute every `Deserialize` type must carry, with its whitespace removed.
const STRICT: &str = r#"feature="strict-contracts",serde(deny_unknown_fields)"#;

fn compact(tokens: &impl ToString) -> String {
    tokens.to_string().split_whitespace().collect()
}

/// Whether `meta` derives `Deserialize`, directly or inside a `cfg_attr`.
fn derives_deserialize(meta: &Meta) -> bool {
    let Meta::List(list) = meta else { return false };
    if list.path.is_ident("derive") {
        return compact(&list.tokens)
            .split(',')
            .any(|derived| derived == "Deserialize" || derived.ends_with("::Deserialize"));
    }
    if list.path.is_ident("cfg_attr") {
        let Ok(args) = list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated) else {
            return false;
        };
        return args.iter().skip(1).any(derives_deserialize);
    }
    false
}

fn is_strict(attr: &Attribute) -> bool {
    matches!(&attr.meta, Meta::List(list)
        if list.path.is_ident("cfg_attr") && compact(&list.tokens) == STRICT)
}

/// `#[serde(deny_unknown_fields)]` with no `cfg_attr`, which would make the
/// shipped app strict too.
fn is_unconditional_deny(attr: &Attribute) -> bool {
    matches!(&attr.meta, Meta::List(list)
        if list.path.is_ident("serde")
            && compact(&list.tokens).split(',').any(|item| item == "deny_unknown_fields"))
}

#[derive(Default)]
struct Scan {
    file: String,
    checked: Vec<String>,
    findings: Vec<String>,
}

impl Scan {
    fn check(&mut self, what: &str, ident: &Ident, attrs: &[Attribute]) {
        if !attrs.iter().any(|attr| derives_deserialize(&attr.meta)) {
            return;
        }
        self.checked.push(ident.to_string());
        if !attrs.iter().any(is_strict) {
            self.findings.push(format!(
                "{}: {what} {ident} derives Deserialize without \
                 #[cfg_attr(feature = \"strict-contracts\", serde(deny_unknown_fields))]",
                self.file
            ));
        }
        if attrs.iter().any(is_unconditional_deny) {
            self.findings.push(format!(
                "{}: {what} {ident} denies unknown fields unconditionally, which makes the \
                 shipped app reject any field a newer server adds",
                self.file
            ));
        }
    }
}

impl<'ast> Visit<'ast> for Scan {
    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        self.check("struct", &item.ident, &item.attrs);
        visit::visit_item_struct(self, item);
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        self.check("enum", &item.ident, &item.attrs);
        visit::visit_item_enum(self, item);
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        let deserialize = item
            .trait_
            .as_ref()
            .and_then(|(path, _)| path.segments.last())
            .is_some_and(|segment| segment.ident == "Deserialize");
        if deserialize {
            self.findings.push(format!(
                "{}: a hand-written Deserialize impl cannot honour strict-contracts; derive it",
                self.file
            ));
        }
        visit::visit_item_impl(self, item);
    }
}

/// Scan one file's source.
fn scan_source(file: &str, source: &str, scan: &mut Scan) {
    let parsed = syn::parse_file(source).unwrap_or_else(|error| panic!("{file}: {error}"));
    scan.file = file.to_owned();
    scan.visit_file(&parsed);
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn every_deserialize_type_in_src_honours_strict_contracts() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files.sort();
    let mut scan = Scan::default();
    for file in &files {
        let source = fs::read_to_string(file).expect("readable source");
        scan_source(&file.display().to_string(), &source, &mut scan);
    }
    assert!(scan.findings.is_empty(), "{}", scan.findings.join("\n"));
    // Not vacuous: the scan found the types, including nested ones.
    for expected in [
        "WorkspaceListResponse",
        "CallAnalysis",
        "PhoneRegion",
        "SetupSteps",
        "PkceVector",
        "TimelineEvent",
        "ContactCompany",
        "BillingDiscount",
        "SchedulingTenant",
        "DeskMessage",
        "RoomE2ee",
        "AiPersona",
        "PersonaVoiceGroup",
        "ManagedAccount",
        "MessagingTestDetails",
    ] {
        assert!(
            scan.checked.iter().any(|name| name == expected),
            "the scan missed {expected}"
        );
    }
    println!(
        "{} Deserialize types checked in {} files",
        scan.checked.len(),
        files.len()
    );
}

// This test crate is built with the feature...
const _: () = assert!(
    cfg!(feature = "strict-contracts"),
    "test builds must enable strict-contracts"
);

#[test]
fn the_library_is_built_with_strict_contracts() {
    // ...and so is the library it tests, which is what the decoding depends on.
    let error = serde_json::from_str::<NativeRevokeResponse>(r#"{"success":true,"extra":1}"#)
        .expect_err("an unknown field must fail in a test build");
    assert!(
        error.to_string().contains("unknown field `extra`"),
        "{error}"
    );
}

// The scan proven able to fail.

fn findings(source: &str) -> Vec<String> {
    let mut scan = Scan::default();
    scan_source("planted.rs", source, &mut scan);
    scan.findings
}

#[test]
fn the_scan_accepts_the_attribute_however_it_is_laid_out() {
    let source = r#"
        #[derive(Debug, serde::Deserialize)]
        #[cfg_attr(
            feature = "strict-contracts",
            serde(deny_unknown_fields)
        )]
        struct A { a: u8 }
        #[derive(Serialize)]
        struct NotDecoded { a: u8 }
    "#;
    assert_eq!(findings(source), Vec::<String>::new());
}

#[test]
fn the_scan_finds_a_type_without_the_attribute() {
    let source = r#"
        mod nested {
            #[derive(
                Clone,
                Deserialize,
            )]
            // #[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
            enum E { A }
        }
    "#;
    let found = findings(source);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(
        found[0].contains("enum E derives Deserialize without"),
        "{found:?}"
    );
}

#[test]
fn the_scan_finds_a_derive_behind_cfg_attr_and_the_wrong_feature() {
    let source = r#"
        #[cfg_attr(test, derive(Deserialize))]
        #[cfg_attr(feature = "other", serde(deny_unknown_fields))]
        struct B;
    "#;
    assert_eq!(findings(source).len(), 1);
}

#[test]
fn the_scan_finds_an_unconditional_deny_and_a_hand_written_impl() {
    let source = r#"
        #[derive(Deserialize)]
        #[cfg_attr(feature = "strict-contracts", serde(deny_unknown_fields))]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct C;
        impl<'de> serde::Deserialize<'de> for D {}
    "#;
    let found = findings(source);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found[0].contains("unconditionally"));
    assert!(found[1].contains("hand-written"));
}
