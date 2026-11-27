//! A test with no `#[test_id]` has no id for a deny-list row to name, so it could never be
//! disabled. This refuses one by file and line, over every `.rs` file under `crates/`.
//!
//! It reads source, so it cannot see a test a derive generates: ts-rs's `export_bindings_*`
//! (one per `#[ts(export)]` type) have no `#[test]` to stamp, carry no id and cannot be denied.
//!
//! `code_only` is not shared with `strip` in `dumpstar-lakehouse/tests/tenant_isolation.rs`:
//! that one is private to an integration target and reads no char or byte-string literal,
//! either of which can hold the quote that would hide every test after it.

use std::path::{Path, PathBuf};

/// The vendored macro is an unmodified upstream copy and holds no test of dumpstar's.
const VENDORED: &str = "spira-test-id";

/// `src` with comments and the contents of string and char literals blanked, newlines kept,
/// so that a `#[test]` quoted in a fixture or a doc comment is not read as an attribute.
fn code_only(src: &str) -> Vec<u8> {
    let s = src.as_bytes();
    let mut out = s.to_vec();
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || !c.is_ascii();
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        let end = if rest.starts_with(b"//") {
            i + rest.iter().position(|&c| c == b'\n').unwrap_or(rest.len())
        } else if rest.starts_with(b"/*") {
            let (mut depth, mut j) = (1, i + 2);
            while j < s.len() && depth > 0 {
                if s[j..].starts_with(b"/*") {
                    (depth, j) = (depth + 1, j + 2);
                } else if s[j..].starts_with(b"*/") {
                    (depth, j) = (depth - 1, j + 2);
                } else {
                    j += 1;
                }
            }
            j
        } else if let Some(end) = raw_string_end(s, i) {
            end
        } else if s[i] == b'"' {
            quoted_end(s, i, b'"')
        } else if s[i] == b'\'' && s.get(i + 1) == Some(&b'\\') {
            quoted_end(s, i, b'\'')
        } else if s[i] == b'\'' {
            // `'x'` is a char literal; `'a` with no closing quote after one char is a lifetime.
            match src.get(i + 1..).and_then(|t| t.chars().next()) {
                Some(c) if s.get(i + 1 + c.len_utf8()) == Some(&b'\'') => i + 2 + c.len_utf8(),
                _ => {
                    i += 1;
                    continue;
                }
            }
        } else {
            // A whole word at once, so the `r` ending an identifier never opens a raw string.
            let word = rest.iter().take_while(|&&c| ident(c)).count();
            let raw_prefix = matches!(&rest[..word], b"r" | b"br" | b"cr");
            i += if raw_prefix || word == 0 { 1 } else { word };
            continue;
        };
        for b in &mut out[i..end.min(s.len())] {
            if *b != b'\n' {
                *b = b' ';
            }
        }
        i = end;
    }
    out
}

/// The end of the raw string whose `r` is at `i`, if one opens there.
fn raw_string_end(s: &[u8], i: usize) -> Option<usize> {
    if s[i] != b'r' {
        return None;
    }
    let hashes = s[i + 1..].iter().take_while(|&&c| c == b'#').count();
    let open = i + 1 + hashes;
    if s.get(open) != Some(&b'"') {
        return None;
    }
    let mut close = vec![b'"'];
    close.resize(1 + hashes, b'#');
    let body = &s[open + 1..];
    let at = body.windows(close.len()).position(|w| w == close);
    Some(at.map_or(s.len(), |at| open + 1 + at + close.len()))
}

/// The index just past the unescaped `quote` that closes the one at `open`.
fn quoted_end(s: &[u8], open: usize, quote: u8) -> usize {
    let mut j = open + 1;
    while j < s.len() {
        match s[j] {
            b'\\' => j += 2,
            c if c == quote => return j + 1,
            _ => j += 1,
        }
    }
    s.len()
}

/// The 1-based line of each `#[test]` or `#[tokio::test]` with no `#[test_id(` among the
/// attributes before it. (One after it would reach the compiler too late to wrap the test.)
fn unstamped_lines(src: &str) -> Vec<usize> {
    let code = code_only(src);
    let skip_ws = |mut i: usize| {
        while code.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        i
    };
    let mut found = Vec::new();
    let mut stamped = false;
    // Where the attribute run being read would continue.
    let mut run_end = 0;
    let mut i = 0;
    while i < code.len() {
        if code[i] != b'#' {
            i += 1;
            continue;
        }
        let open = skip_ws(i + 1);
        if code.get(open) != Some(&b'[') {
            i += 1;
            continue;
        }
        if skip_ws(run_end) != i {
            stamped = false;
        }
        let mut depth = 0;
        let mut end = open;
        while end < code.len() {
            match code[end] {
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ => {}
            }
            end += 1;
            if depth == 0 {
                break;
            }
        }
        let attr: String = code[open + 1..end]
            .iter()
            .filter(|c| !c.is_ascii_whitespace())
            .map(|&c| c as char)
            .collect();
        let attr = attr.strip_prefix("spira_test_id::").unwrap_or(&attr);
        if attr.starts_with("test_id(") {
            stamped = true;
        }
        let is_test = ["test", "tokio::test"].iter().any(|name| {
            attr.strip_prefix(name)
                .is_some_and(|tail| tail.starts_with(']') || tail.starts_with('('))
        });
        if is_test && !stamped {
            found.push(1 + code[..i].iter().filter(|&&c| c == b'\n').count());
        }
        run_end = end;
        i = end;
    }
    found
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("readable source directory") {
        let path = entry.expect("readable directory entry").path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            if name != "target" && name != VENDORED {
                rust_files(&path, out);
            }
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

#[test_id(t8396oo)]
#[test]
fn every_test_carries_an_id() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    files.sort();
    assert!(!files.is_empty(), "no source under {}", crates.display());
    let mut missing = Vec::new();
    for file in &files {
        let src = std::fs::read_to_string(file).expect("readable source file");
        let shown = file.strip_prefix(&crates).unwrap_or(file).display();
        for line in unstamped_lines(&src) {
            missing.push(format!("crates/{shown}:{line}"));
        }
    }
    assert!(
        missing.is_empty(),
        "{} test(s) carry no #[test_id]; run `test-registry stamp --path <file>`:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test_id(tno3ouq)]
#[test]
fn a_test_is_unstamped_unless_an_id_precedes_its_test_attribute() {
    let src = "#[test_id(t000000)]\n#[test]\nfn a() {}\n\
               \x20   #[test]\n    fn b() {}\n\
               #[tokio::test(flavor = \"multi_thread\")]\nasync fn c() {}\n\
               #[test_id(t000001)]\n// why\n#[allow(unused)]\n#[tokio::test]\nasync fn d() {}\n\
               #[test]\n#[test_id(t000002)]\nfn e() {}\n\
               #[cfg(test)]\nmod m {}\n";
    assert_eq!(unstamped_lines(src), [4, 6, 13]);
}

#[test_id(tzy9y2v)]
#[test]
fn a_test_attribute_quoted_in_a_comment_or_literal_is_not_a_test() {
    let quoted = r###"
// #[test]
/* outer /* #[test] */ #[test] */
const A: &str = "\"
#[test]";
const B: &str = r#"" #[test] "#;
const C: &[u8] = br"#[test]";
fn f<'a>(x: &'a str) -> (char, char, char) { ('"', '\'', 'é') }
"###;
    assert!(unstamped_lines(quoted).is_empty());
    let after = format!("{quoted}#[test]\nfn real() {{}}\n");
    assert_eq!(unstamped_lines(&after).len(), 1, "a real one is still seen");
}
