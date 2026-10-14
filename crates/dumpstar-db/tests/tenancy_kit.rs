//! Dumpstar's tenancy kernel is the kit's. This is where that is held to:
//!
//! * the kit's conformance corpora, run through dumpstar's own entry points, so a handle,
//!   `database_id` or email means here exactly what it means in every other consumer;
//! * a guard that the copies this repository was forked with stay deleted.

use std::path::{Path, PathBuf};

use dumpster_kit_conformance as corpus;

#[test]
fn handle_corpus_through_the_validator() {
    for case in corpus::handle_cases() {
        assert_eq!(
            dumpstar_db::validate_tenant_name(&case.value).is_ok(),
            case.valid,
            "handle {:?}",
            case.value
        );
    }
}

#[test]
fn handle_corpus_through_the_registry_check() {
    let conn = dumpstar_db::open_registry(Path::new(":memory:")).unwrap();
    for case in corpus::handle_cases() {
        assert_eq!(
            dumpstar_db::registry::insert(&conn, &case.value).is_ok(),
            case.valid,
            "handle {:?}",
            case.value
        );
    }
}

#[test]
fn database_id_corpus_through_the_validator() {
    for case in corpus::ulid_cases() {
        assert_eq!(
            dumpstar_db::validate_database_id(&case.value).is_ok(),
            case.valid,
            "database_id {:?}",
            case.value
        );
    }
}

#[test]
fn email_corpus_through_the_normaliser() {
    for case in corpus::email_cases() {
        let normal =
            dumpstar_db::registry::normalise_email(&case.value).is_some_and(|n| n == case.value);
        assert_eq!(normal, case.valid, "email {:?}", case.value);
    }
}

fn rust_and_sql_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_and_sql_under(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "sql")
        ) {
            out.push(path);
        }
    }
}

/// The tenancy kernel has one implementation. These are the fingerprints of the copies
/// this repository was forked with; none may reappear under `crates/`.
#[test]
fn the_forked_tenancy_copies_are_gone() {
    const COPIES: &[(&str, &str)] = &[
        ("fn validate_tenant_name", "the handle validator"),
        ("fn validate_database_id", "the database_id validator"),
        (
            "CREATE TABLE IF NOT EXISTS user_identity",
            "the registry schema",
        ),
        ("Algorithm::RS256", "Access JWT verification"),
        ("fn assert_isolated", "the isolated-connection assertion"),
        ("fn gate_attached", "the schema-version gate"),
        ("fn lookup_by_email", "the registry's email lookup"),
        ("struct TenantId", "tenant resolution"),
    ];
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_and_sql_under(&crates, &mut files);
    let me = Path::new(file!()).file_name().unwrap();
    let mut found = Vec::new();
    for file in files {
        if file.file_name() == Some(me) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for (needle, what) in COPIES {
            if text.contains(needle) {
                found.push(format!("{}: {what} ({needle})", file.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "tenancy code lives in dumpster-kit-tenancy, not here:\n{}",
        found.join("\n")
    );
}
