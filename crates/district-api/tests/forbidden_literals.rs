//! A scan of every crate's `src/` for surfaces this client must never reach.
//!
//! - `/api/admin`: service administration, which is web-only.
//! - `calls/outbound`: bulk dialling, which puts an AI agent on the line.
//! - `elevate`: the administrator step-up sign-in, removed from the service.
//! - `video_`: the prefix of AI video avatar rooms, which are billed per session.
//! - a JSON field named `intent`: how a sign-in asks for administrator rights.
//!
//! The endpoint table already has no such endpoint, and the parity test keeps it
//! that way. This scan is the second line: it catches the same thing typed
//! anywhere else, in any crate, including a hand-built URL or a doc comment that
//! would invite one. The first four are matched case-insensitively, in the
//! source as written and in every string literal as the compiler reads it (its
//! escapes decoded, its line continuations joined, and the pieces of a
//! `concat!` put together), so spelling one in pieces does not hide it.
//!
//! One exception: a string literal inside the initializer of [`EXCLUDED`] whose
//! whole value is one of that list's names, paths or reasons, because the
//! exclusion list has to be able to name what it excludes. Both conditions are
//! checked, the place and the value, so the same text anywhere else, even a copy
//! of the same string next to the list, is a finding.
//!
//! This file is outside every `src/`, so the scan does not read it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use district_api::EXCLUDED;

const FORBIDDEN: &[&str] = &["/api/admin", "calls/outbound", "elevate", "video_"];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    file: String,
    line: usize,
    what: String,
}

/// A string literal: where it sits in the source, and its value.
struct Literal {
    start: usize,
    end: usize,
    value: String,
}

/// The string literals in Rust source, with their escapes decoded. Comments are
/// skipped (they are scanned as plain text, never allowed), and a character
/// literal such as `'"'` is not mistaken for the start of a string.
fn string_literals(source: &str) -> Vec<Literal> {
    lex(source).0
}

/// The string literals in Rust source, and where its comments are.
fn lex(source: &str) -> (Vec<Literal>, Vec<(usize, usize)>) {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut comments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                let start = i;
                i = source[i..].find('\n').map_or(bytes.len(), |n| i + n);
                comments.push((start, i));
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                let start = i;
                let mut depth = 0;
                while i < bytes.len() {
                    if source[i..].starts_with("/*") {
                        depth += 1;
                        i += 2;
                    } else if source[i..].starts_with("*/") {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                comments.push((start, i.min(bytes.len())));
            }
            b'\'' => {
                // A character literal is a quote, one character or escape, and a
                // quote. Anything else is a lifetime or a label.
                let rest = &source[i + 1..];
                let len = if rest.starts_with('\\') {
                    rest.get(2..).and_then(|r| r.find('\'')).map(|n| n + 2)
                } else {
                    rest.chars()
                        .next()
                        .map(char::len_utf8)
                        .filter(|n| rest[*n..].starts_with('\''))
                };
                i += len.map_or(1, |n| n + 2);
            }
            b'r' if raw_string_start(&source[i..]).is_some() => {
                let hashes = raw_string_start(&source[i..]).unwrap_or_default();
                let open = i + 1 + hashes + 1;
                let close_marker = format!("\"{}", "#".repeat(hashes));
                let close = source[open..]
                    .find(&close_marker)
                    .map_or(bytes.len(), |n| open + n);
                literals.push(Literal {
                    start: i,
                    end: close + close_marker.len(),
                    value: source[open..close].to_owned(),
                });
                i = close + close_marker.len();
            }
            b'"' => {
                let (value, end) = cooked_string(source, i + 1);
                literals.push(Literal {
                    start: i,
                    end,
                    value,
                });
                i = end;
            }
            _ => i += 1,
        }
    }
    (literals, comments)
}

/// The source with its comments and string literals blanked out, every other
/// byte where it was: the code alone, to look for a field name in.
fn code_only(source: &str) -> String {
    let (literals, comments) = lex(source);
    let mut code = source.as_bytes().to_vec();
    let spans = literals
        .iter()
        .map(|l| (l.start, l.end))
        .chain(comments.iter().copied());
    for (start, end) in spans {
        for byte in &mut code[start..end.min(source.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    // Only ASCII bytes were replaced, by ASCII, so this is still UTF-8 except
    // inside a blanked span, whose bytes were all replaced.
    String::from_utf8_lossy(&code).into_owned()
}

/// The values of the string literals each `concat!` joins, with where each
/// invocation starts.
fn concatenations(source: &str, literals: &[Literal]) -> Vec<(usize, String)> {
    let code = code_only(source);
    let mut joined = Vec::new();
    for (at, _) in code.match_indices("concat!") {
        let Some(open) = code[at..].find(['(', '[', '{']).map(|n| at + n) else {
            continue;
        };
        let mut depth = 0;
        let mut close = code.len();
        for (offset, c) in code[open..].char_indices() {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = open + offset;
                        break;
                    }
                }
                _ => {}
            }
        }
        let value: String = literals
            .iter()
            .filter(|l| open < l.start && l.start < close)
            .map(|l| l.value.as_str())
            .collect();
        joined.push((at, value));
    }
    joined
}

/// Whether `code` has a field or a key named `intent` at `offset`: the whole
/// word, followed by one colon (not the two of a path).
fn names_intent_field(code: &str, offset: usize) -> bool {
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let before = code[..offset].chars().next_back();
    let after = code[offset + "intent".len()..].trim_start();
    !before.is_some_and(word) && after.starts_with(':') && !after.starts_with("::")
}

/// The number of `#` in a raw string opening at the start of `s` (`r"`, `r#"`).
/// A raw identifier such as `r#type` is not one.
fn raw_string_start(s: &str) -> Option<usize> {
    let after = &s[1..];
    let hashes = after.len() - after.trim_start_matches('#').len();
    after[hashes..].starts_with('"').then_some(hashes)
}

/// Decodes a `"..."` literal whose body starts at `from`. Returns the value and
/// the offset just past the closing quote.
fn cooked_string(source: &str, from: usize) -> (String, usize) {
    let mut value = String::new();
    let mut chars = source[from..].char_indices().peekable();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return (value, from + offset + 1),
            '\\' => match chars.next() {
                Some((_, 'n')) => value.push('\n'),
                Some((_, 't')) => value.push('\t'),
                Some((_, 'r')) => value.push('\r'),
                Some((_, '0')) => value.push('\0'),
                Some((_, '\n')) => while chars.next_if(|(_, c)| c.is_whitespace()).is_some() {},
                Some((_, 'x')) => {
                    let hex: String = (0..2)
                        .filter_map(|_| chars.next().map(|(_, c)| c))
                        .collect();
                    value.extend(u8::from_str_radix(&hex, 16).ok().map(char::from));
                }
                Some((_, 'u')) => {
                    let mut hex = String::new();
                    for (_, c) in chars.by_ref() {
                        match c {
                            '{' => {}
                            '}' => break,
                            c => hex.push(c),
                        }
                    }
                    value.extend(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32));
                }
                Some((_, other)) => value.push(other),
                None => break,
            },
            other => value.push(other),
        }
    }
    (value, source.len())
}

/// Where the initializer of `const EXCLUDED` is, from its `[` to its `]`.
fn exclusion_list_span(source: &str, literals: &[Literal]) -> Option<(usize, usize)> {
    const HEAD: &str = "const EXCLUDED: &[Exclusion] = &";
    let open = source.find(HEAD)? + HEAD.len();
    let mut depth = 0;
    let mut i = open;
    while i < source.len() {
        if let Some(literal) = literals.iter().find(|l| l.start == i) {
            i = literal.end;
            continue;
        }
        match source.as_bytes()[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open, i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The findings in one file's source. `allowed` is the set of whole literal
/// values that may contain a forbidden text inside the exclusion list.
fn scan(file: &str, source: &str, allowed: &BTreeSet<String>) -> Vec<Finding> {
    let literals = string_literals(source);
    let line_of = |offset: usize| source[..offset].matches('\n').count() + 1;
    let list = exclusion_list_span(source, &literals);
    let exempt = |offset: usize| {
        let in_list = list.is_some_and(|(start, end)| start < offset && offset < end);
        in_list
            && literals
                .iter()
                .any(|l| l.start < offset && offset < l.end && allowed.contains(&l.value))
    };
    let lower = source.to_ascii_lowercase();
    let mut findings = Vec::new();
    let mut found = |offset: usize, what: &str| {
        let finding = Finding {
            file: file.to_owned(),
            line: line_of(offset),
            what: what.to_owned(),
        };
        if !findings.contains(&finding) {
            findings.push(finding);
        }
    };

    // As written, comments included, and as the compiler reads each literal and
    // each `concat!`.
    let decoded = literals
        .iter()
        .filter(|l| !exempt(l.start + 1))
        .map(|l| (l.start, l.value.clone()))
        .chain(concatenations(source, &literals));
    for needle in FORBIDDEN {
        for (offset, _) in lower.match_indices(needle) {
            if !exempt(offset) {
                found(offset, needle);
            }
        }
    }
    for (offset, value) in decoded {
        let value = value.to_ascii_lowercase();
        for needle in FORBIDDEN.iter().filter(|needle| value.contains(**needle)) {
            found(offset, needle);
        }
    }

    // An `intent` JSON key, written as a literal (`json!({"intent": ..})`,
    // `rename = "intent"`) or as a field of any visibility, anywhere on its line,
    // that serializes under that name.
    for literal in literals.iter().filter(|l| l.value == "intent") {
        found(literal.start, "an intent field");
    }
    let code = code_only(source);
    for (offset, _) in code.match_indices("intent") {
        if names_intent_field(&code, offset) {
            found(offset, "an intent field");
        }
    }
    findings
}

/// The strings the exclusion list is made of.
fn allowed() -> BTreeSet<String> {
    EXCLUDED
        .iter()
        .flat_map(|x| [x.name, x.path.as_str(), x.reason])
        .map(str::to_owned)
        .collect()
}

/// Every `.rs` file under `root/crates/*/src/`, sorted.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("readable directory") {
            let path = entry.expect("readable entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    for krate in fs::read_dir(root.join("crates")).expect("a crates/ directory") {
        let src = krate.expect("readable entry").path().join("src");
        if src.is_dir() {
            walk(&src, &mut files);
        }
    }
    files.sort();
    files
}

fn scan_tree(root: &Path) -> Vec<Finding> {
    let allowed = allowed();
    rust_sources(root)
        .iter()
        .flat_map(|path| {
            let name = path
                .strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string();
            scan(
                &name,
                &fs::read_to_string(path).expect("UTF-8 source"),
                &allowed,
            )
        })
        .collect()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn no_crate_names_a_forbidden_surface() {
    let findings = scan_tree(&workspace_root());
    let report: Vec<_> = findings
        .iter()
        .map(|f| format!("  {}:{}: {}", f.file, f.line, f.what))
        .collect();
    assert!(
        findings.is_empty(),
        "forbidden text in src/:\n{}",
        report.join("\n")
    );
}

/// The walk has to reach every crate, or a clean result means nothing.
#[test]
fn the_scan_reads_every_crate() {
    let root = workspace_root();
    let files = rust_sources(&root);
    let crates: BTreeSet<_> = files
        .iter()
        .filter_map(|f| f.strip_prefix(root.join("crates")).ok())
        .filter_map(|f| f.iter().next())
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    let members = workspace_members(&fs::read_to_string(root.join("Cargo.toml")).unwrap());
    // Read by a TOML parser, so a members list written on one line (or any other
    // way TOML allows) is still read, and an empty one fails here rather than
    // checking nothing.
    assert!(!members.is_empty(), "the workspace lists no members");
    assert_eq!(
        members, crates,
        "every member's src/ is scanned, and every crate scanned is a member"
    );
    let this_crate = root.join("crates/district-api/src");
    for file in ["lib.rs", "exclusions.rs"] {
        assert!(
            files.contains(&this_crate.join(file)),
            "{file} is not scanned"
        );
    }
}

/// The workspace's members, as `crates/<name>` entries name them.
fn workspace_members(manifest: &str) -> BTreeSet<String> {
    let manifest: toml::Table = manifest.parse().expect("Cargo.toml is TOML");
    manifest["workspace"]["members"]
        .as_array()
        .expect("a members list")
        .iter()
        .map(|member| {
            let member = member.as_str().expect("a member path");
            member
                .strip_prefix("crates/")
                .unwrap_or_else(|| panic!("{member} is not under crates/"))
                .to_owned()
        })
        .collect()
}

#[test]
fn the_members_are_read_however_the_list_is_written() {
    let one_line = "[workspace]\nmembers = [\"crates/a\", \"crates/b\"]\n";
    let spread =
        "[workspace]\nresolver = \"3\"\nmembers = [\n  \"crates/b\",\n  \"crates/a\",\n]\n";
    for manifest in [one_line, spread] {
        assert_eq!(
            workspace_members(manifest),
            BTreeSet::from(["a".to_owned(), "b".to_owned()])
        );
    }
}

/// The exclusion list really does contain the forbidden texts, so the real scan
/// passing proves the exemption works, not merely that nothing was there.
#[test]
fn the_exclusion_list_is_exempt_by_value() {
    let exclusions =
        fs::read_to_string(workspace_root().join("crates/district-api/src/exclusions.rs")).unwrap();
    let everything_allowed = scan("exclusions.rs", &exclusions, &BTreeSet::new());
    let hit: BTreeSet<_> = everything_allowed.iter().map(|f| f.what.as_str()).collect();
    for needle in ["/api/admin", "calls/outbound", "elevate"] {
        assert!(
            hit.contains(needle),
            "exclusions.rs no longer names {needle}"
        );
    }
    assert!(scan("exclusions.rs", &exclusions, &allowed()).is_empty());
}

#[test]
fn a_planted_violation_is_caught_in_code_strings_and_comments() {
    let allowed = allowed();
    let planted = [
        (
            "let url = format!(\"{base}/api/admin/users\");",
            "/api/admin",
        ),
        ("// POST to /API/ADMIN/workspaces", "/api/admin"),
        (
            "client.post(\"/api/district/calls/outbound\")",
            "calls/outbound",
        ),
        ("fn elevate_session() {}", "elevate"),
        ("/// Calls the Elevate endpoint.", "elevate"),
        ("let room = \"video_ws_1_avatar\";", "video_"),
        ("const PREFIX: &str = r#\"VIDEO_\"#;", "video_"),
        // Spelled in pieces, the compiler still reads the forbidden text.
        ("let url = \"/api/\\u{61}dmin/users\";", "/api/admin"),
        ("let url = \"/api/\\x61dmin/users\";", "/api/admin"),
        ("let url = \"/api/ad\\\n    min/users\";", "/api/admin"),
        ("let p = concat!(\"/api/\", \"admin\");", "/api/admin"),
        (
            "let p = concat![\"calls/\", \"out\", \"bound\"];",
            "calls/outbound",
        ),
    ];
    for (source, needle) in planted {
        let findings = scan("planted.rs", source, &allowed);
        assert!(
            findings.iter().any(|f| f.what == needle && f.line == 1),
            "{source:?} was not caught as {needle}: {findings:?}"
        );
    }
}

#[test]
fn only_a_whole_exclusion_value_inside_the_list_is_exempt() {
    let allowed = allowed();
    assert!(
        allowed.contains("/api/admin"),
        "the administration exclusion"
    );
    let list = "pub const EXCLUDED: &[Exclusion] = &[Exclusion {\n    \
                path: PathMatch::Subtree(\"/api/admin\"),\n    \
                note: \"see /api/admin\",\n}];\n";
    let findings = scan("list.rs", list, &allowed);
    assert_eq!(
        findings.len(),
        1,
        "only the value that is not an exclusion's: {findings:?}"
    );
    assert_eq!(findings[0].line, 3);

    for source in [
        "const A: &str = \"/api/admin\";",
        "const A: &str = \"/api/admin/overview\";",
        "// \"/api/admin\" in a comment is still a mention",
        "const EXCLUDED: &[Exclusion] = &[ \"unterminated\"; const A: &str = \"/api/admin\";",
    ] {
        assert_eq!(scan("bad.rs", source, &allowed).len(), 1, "{source}");
    }
}

#[test]
fn an_intent_field_is_caught_and_the_word_alone_is_not() {
    let allowed = allowed();
    for source in [
        "let body = json!({ \"intent\": \"admin\" });",
        "#[serde(rename = \"intent\")]",
        "    pub intent: String,",
        "    intent: Option<String>,",
        "    pub(crate) intent: u8,",
        "    pub(super) intent: String,",
        "struct SignIn { intent: &'static str }",
        "#[derive(Serialize)] struct B<'a> { pub intent: &'a str }",
        "let body = Body { kind, intent : admin };",
    ] {
        let findings = scan("planted.rs", source, &allowed);
        assert!(
            findings.iter().any(|f| f.what == "an intent field"),
            "{source:?} was not caught: {findings:?}"
        );
    }
    for source in [
        "// The intent of this module is clarity.",
        "let intentional = \"intentional\";",
        "let s = \"the intent\";",
        "/* intent: in a block comment */",
        "let s = \"intent: in a string\";",
        "use crate::intent::Thing;",
        "let my_intent: u8 = 0;",
    ] {
        assert!(scan("fine.rs", source, &allowed).is_empty(), "{source:?}");
    }
}

#[test]
fn literals_are_read_the_way_the_compiler_reads_them() {
    let source = concat!(
        "let q = '\"'; let e = '\\''; fn f<'a>(x: &'a str) {}\n",
        "/* a /* nested \"not a string\" */ comment */\n",
        "let a = \"one \\\"two\\\" \\\\ three\\n\";\n",
        "let b = r##\"raw \"# still\"##;\n",
        "let c = \"joined \\\n      here\";\n",
        "let d = \"unterminated",
    );
    let values: Vec<_> = string_literals(source)
        .into_iter()
        .map(|l| l.value)
        .collect();
    assert_eq!(
        values,
        [
            "one \"two\" \\ three\n",
            "raw \"# still",
            "joined here",
            "unterminated",
        ]
    );
    assert_eq!(
        string_literals("let t = \"tab\\t\"; let x = \"\\")[0].value,
        "tab\t"
    );
    let escapes = string_literals("\"\\r\\0\\x41\\u{1F427}\\u{110000}\\xZZ\"");
    assert_eq!(escapes[0].value, "\r\0A\u{1F427}");
    // A `concat!` with no brackets after it joins nothing.
    assert_eq!(
        concatenations("concat! \"a\"", &string_literals("concat! \"a\"")),
        []
    );
}

/// A planted crate on disk, to prove the walk finds files in nested modules and
/// the scan reports them with their paths.
#[test]
fn the_walk_finds_a_planted_file_in_a_nested_module() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("forbidden-literals-plant");
    let _ = fs::remove_dir_all(&root);
    let nested = root.join("crates/planted/src/deep");
    fs::create_dir_all(&nested).unwrap();
    fs::create_dir_all(root.join("crates/no-src")).unwrap();
    fs::write(root.join("crates/planted/src/lib.rs"), "mod deep;\n").unwrap();
    fs::write(
        root.join("crates/planted/src/notes.txt"),
        "video_ is not Rust\n",
    )
    .unwrap();
    fs::write(nested.join("mod.rs"), "\nconst ROOM: &str = \"video_1\";\n").unwrap();

    let findings = scan_tree(&root);
    fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        findings,
        [Finding {
            file: "crates/planted/src/deep/mod.rs".to_owned(),
            line: 2,
            what: "video_".to_owned(),
        }]
    );
}
