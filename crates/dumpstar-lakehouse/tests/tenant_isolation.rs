//! Tenant identity stays out of the CATALOG zone (pd-cgi9, pd-7x83; restored
//! in Rust by sp-kzof8i after the Python lake's shell guard was deleted).
//!
//! The lake bucket has two zones: the catalog zone (`raw/`, `lake/` —
//! cross-tenant, shared) and the tenant zone (`tenant/` — keyed by
//! `database_id` by construction). The rules run opposite ways:
//!
//!   catalog zone  must never be keyed by a tenant
//!   tenant zone   must always be keyed by a tenant, resolve none, and never
//!                 reach the catalog
//!
//! Layout and key builders are the kit's (`dumpster-kit-lake`), so the checks
//! are of two kinds: behavioural ones over the kit's real key builders, and
//! source scans of the kit's lake and ship crates at the pinned revision plus
//! this workspace's lakehouse and ship crates, over comment-stripped code.
//! A bump of the kit pin is what re-proves the kit half. The Iceberg schemas
//! the old guard scanned are gone with the Python lake: nothing in this
//! workspace writes Iceberg, and the kit's lake crate carries none (section 6
//! asserts both).
//!
//! Every scan fails closed: a corpus that is empty is a failure, and every
//! source file of the kit's lake crate must be classified into exactly one
//! zone, so a new file is a zone decision that fails here until made.
//!
//! The credential half of the boundary (the catalog role cannot read
//! `tenant/`) is an IAM property against a real bucket and is not here.

use dumpster_kit_lake::keys::{self, PartFormat, Source};
use dumpster_kit_lake::tenant::{self, Dataset as TenantDataset};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Source identifiers that reach a tenant.
const IDENT: &[&str] = &[
    "tenant",
    "database_id",
    "user_db",
    "registry",
    "collection.sqlite",
    "DUMPSTAR_USER",
];
/// IDENT minus what the tenant zone may say (its own name, and the id it is
/// handed), plus what it must not reach: it partitions by an id, it never
/// looks one up.
const RESOLVE: &[&str] = &[
    "user_db",
    "registry",
    "collection.sqlite",
    "DUMPSTAR_USER",
    "tenants_dir",
    "tenant_db_file",
    "tenant_db_path",
    "rusqlite",
    "dumpstar_db",
];
const RESOLVING_API: &[&str] = &["tenant_db_file", "tenant_db_path", "tenants_dir"];

const CATALOG_ZONE_RS: &[&str] = &[
    "error.rs",
    "keys.rs",
    "manifest.rs",
    "reader.rs",
    "sink.rs",
    "store.rs",
];
const TENANT_ZONE_RS: &[&str] = &["tenant.rs"];
const ZONE_ROOT_RS: &[&str] = &["lib.rs", "config.rs"];

const ONLINE_CRATES: &[&str] = &["dumpstar-db", "dumpstar-server"];

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

/// The source directory of a package in the workspace graph, at the pinned rev.
/// One `cargo metadata` per process: the tests share the answer.
fn package_dir(name: &str) -> PathBuf {
    static META: OnceLock<serde_json::Value> = OnceLock::new();
    let meta = META.get_or_init(|| {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let out = Command::new(cargo)
            .args(["metadata", "--format-version", "1", "--locked"])
            .current_dir(repo())
            .output()
            .expect("run cargo metadata");
        assert!(out.status.success(), "cargo metadata failed");
        serde_json::from_slice(&out.stdout).expect("metadata json")
    });
    let manifest = meta["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .find(|p| p["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not in the workspace graph"))["manifest_path"]
        .as_str()
        .expect("manifest_path");
    Path::new(manifest).parent().unwrap().to_path_buf()
}

/// Rust source with comments removed; string literals blanked unless `keep`.
fn strip(src: &str, keep: bool) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let n = b.get(i + 1).copied().unwrap_or('\0');
        if c == '/' && n == '/' {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && n == '*' {
            let mut depth = 1;
            i += 2;
            while i < b.len() && depth > 0 {
                if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                    depth += 1;
                    i += 2;
                } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                    depth -= 1;
                    i += 2;
                } else {
                    if b[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
            }
        } else if c == 'r' && (n == '"' || n == '#') && {
            let mut j = i + 1;
            while b.get(j) == Some(&'#') {
                j += 1;
            }
            b.get(j) == Some(&'"')
        } {
            let mut j = i + 1;
            let mut h = 0;
            while b[j] == '#' {
                h += 1;
                j += 1;
            }
            j += 1;
            loop {
                if j >= b.len() {
                    break;
                }
                if b[j] == '"' && (0..h).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                    j += 1 + h;
                    break;
                }
                if keep || b[j] == '\n' {
                    out.push(b[j]);
                }
                j += 1;
            }
            i = j;
        } else if c == '"' {
            out.push('"');
            i += 1;
            while i < b.len() && b[i] != '"' {
                if b[i] == '\\' {
                    if keep {
                        out.push(b[i]);
                        out.push(b.get(i + 1).copied().unwrap_or(' '));
                    }
                    i += 2;
                    continue;
                }
                if keep || b[i] == '\n' {
                    out.push(b[i]);
                }
                i += 1;
            }
            out.push('"');
            i += 1;
        } else if c == '\'' && n != '\0' && b.get(i + 2) == Some(&'\'') {
            // a char literal such as '"' must not open a string
            i += 3;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(&dir.join("src"), &mut v);
    v.sort();
    v
}

/// Lines of `files` (comment-stripped) containing any of `needles`,
/// case-insensitively, as `file:line: text`.
fn scan(needles: &[&str], files: &[PathBuf], keep: bool) -> Vec<String> {
    let mut hits = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("read {f:?}: {e}"));
        for (n, line) in strip(&src, keep).lines().enumerate() {
            let l = line.to_lowercase();
            if needles.iter().any(|x| l.contains(&x.to_lowercase())) {
                hits.push(format!("{}:{}: {}", f.display(), n + 1, line.trim()));
            }
        }
    }
    hits
}

fn none(label: &str, hits: Vec<String>) {
    assert!(hits.is_empty(), "{label}:\n  {}", hits.join("\n  "));
}

fn some(label: &str, hits: Vec<String>) {
    assert!(
        !hits.is_empty(),
        "{label}: found nothing, so the rule is asserting nothing"
    );
}

fn in_dir(dir: &Path, names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|n| dir.join("src").join(n)).collect()
}

/// Dependency lines of a manifest, dev-dependencies excluded.
fn normal_deps(toml: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(toml).unwrap_or_else(|e| panic!("read {toml:?}: {e}"));
    let end = text.find("[dev-dependencies]").unwrap_or(text.len());
    text[..end].lines().map(|l| l.trim().to_string()).collect()
}

fn dep_hits(toml: &Path, names: &[&str]) -> Vec<String> {
    normal_deps(toml)
        .into_iter()
        .filter(|l| {
            names.iter().any(|n| {
                l.strip_prefix(n)
                    .is_some_and(|r| r.trim_start().starts_with(['.', '=']))
            })
        })
        .map(|l| format!("{}: {l}", toml.display()))
        .collect()
}

#[test]
fn lake_crate_cannot_open_a_database() {
    let lake = package_dir("dumpster-kit-lake");
    let hits = dep_hits(
        &lake.join("Cargo.toml"),
        &[
            "rusqlite",
            "dumpstar-db",
            "libsqlite3-sys",
            "sqlx",
            "diesel",
        ],
    );
    none("the kit's lake crate depends on a SQLite crate", hits);
}

#[test]
fn catalog_zone_names_no_tenant() {
    let lake = package_dir("dumpster-kit-lake");
    let mut needles = IDENT.to_vec();
    needles.extend(["rusqlite", "dumpstar_db"]);
    let files = in_dir(&lake, CATALOG_ZONE_RS);
    assert!(
        files.iter().all(|f| f.is_file()),
        "a classified file is missing"
    );
    none(
        "a catalog-zone module names a tenant",
        scan(&needles, &files, false),
    );
}

#[test]
fn offline_derive_calls_no_tenant_resolving_api() {
    let mut needles = IDENT.to_vec();
    needles.extend(RESOLVING_API);
    let files = rust_sources(&repo().join("crates/dumpstar-lakehouse"));
    assert!(!files.is_empty());
    // This file is the guard itself and names every term on purpose.
    let files: Vec<_> = files
        .into_iter()
        .filter(|f| f.file_name().is_some_and(|n| n != "tenant_isolation.rs"))
        .collect();
    none(
        "dumpstar-lakehouse names a tenant or the registry",
        scan(&needles, &files, false),
    );
}

#[test]
fn catalog_keys_carry_no_tenant_and_tenant_keys_always_do() {
    let mut catalog = Vec::new();
    for &d in keys::Dataset::ALL {
        for f in [PartFormat::Json, PartFormat::Csv, PartFormat::TarGz] {
            catalog.push(keys::part_key(
                Source::Tcgcsv,
                d,
                "2026-10-16",
                "01JABCDEFGHJKMNPQRSTVWXYZ0",
                3,
                f,
            ));
        }
        catalog.push(keys::manifest_key(
            Source::Tcgcsv,
            d,
            "2026-10-16",
            "01JABCDEFGHJKMNPQRSTVWXYZ0",
        ));
        catalog.push(keys::run_prefix(
            Source::Tcgcsv,
            d,
            "2026-10-16",
            "01JABCDEFGHJKMNPQRSTVWXYZ0",
        ));
    }
    assert!(!catalog.is_empty());
    for k in &catalog {
        assert!(tenant::is_catalog_key(k), "{k} is not a catalog key");
        assert!(!tenant::is_tenant_key(k), "{k} is also a tenant key");
        assert!(
            !k.contains("database_id") && !k.contains("tenant"),
            "{k} names a tenant"
        );
    }

    let id = "tenant-under-test-0001";
    let mut tenant_keys = vec![tenant::tenant_prefix(id).unwrap()];
    for &d in TenantDataset::ALL {
        tenant_keys.push(tenant::dataset_prefix(id, d).unwrap());
        tenant_keys.push(tenant::partition_prefix(id, d, "2026-10-16").unwrap());
        tenant_keys.push(tenant::part_key(id, d, "2026-10-16", 0).unwrap());
        tenant_keys.push(tenant::range_part_key(id, d, "2026-10-16", 1, 9).unwrap());
    }
    for k in &tenant_keys {
        assert!(tenant::is_tenant_key(k), "{k} is not a tenant key");
        assert!(!tenant::is_catalog_key(k), "{k} is also a catalog key");
        assert!(k.contains(id), "{k} is not keyed by the tenant");
        assert!(
            tenant::tenant_prefix("").is_err(),
            "an empty id must be refused"
        );
    }
}

#[test]
fn zone_roots_do_not_contain_one_another() {
    let mut roots = vec![tenant::TENANT_ROOT];
    roots.extend(tenant::CATALOG_ROOTS);
    assert!(roots.len() >= 3);
    for a in &roots {
        for b in &roots {
            assert!(
                a == b || !b.starts_with(a),
                "zone root {a:?} contains {b:?}"
            );
        }
    }
}

#[test]
fn no_iceberg_schema_exists_to_carry_a_tenant_column() {
    let mut files = rust_sources(&package_dir("dumpster-kit-lake"));
    files.extend(rust_sources(&package_dir("dumpster-kit-lakehouse")));
    files.extend(rust_sources(&repo().join("crates/dumpstar-lakehouse")));
    files.retain(|f| f.file_name().is_some_and(|n| n != "tenant_isolation.rs"));
    assert!(!files.is_empty());
    none(
        "an Iceberg schema is defined where this guard does not scan its fields",
        scan(
            &["NestedField", "iceberg::spec", "iceberg_rust"],
            &files,
            true,
        ),
    );
}

#[test]
fn tenant_zone_is_keyed_by_tenant_and_resolves_none() {
    let lake = package_dir("dumpster-kit-lake");
    let tenant_rs = in_dir(&lake, TENANT_ZONE_RS);
    none(
        "the tenant zone resolves a tenant identity",
        scan(RESOLVE, &tenant_rs, false),
    );
    let roots = in_dir(&lake, ZONE_ROOT_RS);
    none(
        "the crate root resolves a tenant identity",
        scan(RESOLVE, &roots, false),
    );

    // Every `pub fn ... -> Result<String>` in the zone takes a database_id.
    let code = strip(&std::fs::read_to_string(&tenant_rs[0]).unwrap(), false);
    let mut builders = 0;
    for part in code.split("pub fn ").skip(1) {
        let sig = part.split('{').next().unwrap();
        if sig.contains("-> Result<String>") {
            builders += 1;
            assert!(
                sig.contains("database_id"),
                "key builder takes no database_id: pub fn {sig}"
            );
        }
    }
    assert!(
        builders >= 4,
        "found {builders} tenant-zone key builders; the parser reads nothing"
    );
}

#[test]
fn shipper_writes_tenant_zone_and_reaches_no_catalog_prefix() {
    let mut files = rust_sources(&repo().join("crates/dumpstar-ship"));
    files.extend(rust_sources(&package_dir("dumpster-kit-ship")));
    assert!(files.len() > 2);
    // Object keys are string literals, so the strings are kept here.
    none(
        "a catalog-zone prefix appears in the shipper",
        scan(&["\"raw/", "\"lake/"], &files, true),
    );
    none(
        "the shipper calls a catalog-zone entry point",
        scan(
            &[
                "dumpster_kit_lake::keys",
                "dumpster_kit_lake::sink",
                "dumpster_kit_lake::manifest",
                "dumpster_kit_lake::reader",
                "dumpster_kit_lake::RawLanding",
                "dumpster_kit_lake::RawZone",
                "RawLanding",
                "RawZone",
            ],
            &files,
            false,
        ),
    );
    none(
        "the shipper names a catalog credential",
        scan(&["AWS_PROFILE", "KEY_CATALOG_PROFILE"], &files, true),
    );
    some(
        "the shipper opens the tenant zone",
        scan(&["open_tenant_zone"], &files, false),
    );
    some(
        "and builds its keys from the tenant layout module",
        scan(
            &[
                "range_part_key",
                "part_key",
                "tenant_prefix",
                "partition_prefix",
            ],
            &files,
            false,
        ),
    );
}

#[test]
fn online_path_links_neither_zone() {
    let mut found = 0;
    for c in ONLINE_CRATES {
        let toml = repo().join("crates").join(c).join("Cargo.toml");
        assert!(toml.is_file(), "{c} not found");
        found += 1;
        none(
            "an online crate depends on a lake crate or the S3 SDK",
            dep_hits(
                &toml,
                &[
                    "dumpster-kit-lake",
                    "dumpster-kit-ship",
                    "dumpstar-lakehouse",
                    "dumpstar-ship",
                    "aws-sdk-s3",
                    "aws-config",
                ],
            ),
        );
    }
    assert_eq!(found, ONLINE_CRATES.len());
}

#[test]
fn the_two_zones_do_not_reach_each_other() {
    let lake = package_dir("dumpster-kit-lake");
    none(
        "a catalog-zone module reaches the tenant zone",
        scan(
            &[
                "tenant::",
                "TENANT_ROOT",
                "TenantZone",
                "open_tenant_zone",
                "TenantDataset",
            ],
            &in_dir(&lake, CATALOG_ZONE_RS),
            false,
        ),
    );
    none(
        "the tenant zone builds a catalog key",
        scan(
            &[
                "crate::keys",
                "crate::sink",
                "crate::manifest",
                "crate::reader",
                "RawLanding",
                "Source::",
            ],
            &in_dir(&lake, TENANT_ZONE_RS),
            false,
        ),
    );
    let derive: Vec<_> = rust_sources(&repo().join("crates/dumpstar-lakehouse"))
        .into_iter()
        .filter(|f| f.file_name().is_some_and(|n| n != "tenant_isolation.rs"))
        .collect();
    none(
        "the offline derive reaches a tenant-zone entry point",
        scan(
            &[
                "open_tenant_zone",
                "TENANT_ROOT",
                "TenantZone",
                "range_part_key",
            ],
            &derive,
            false,
        ),
    );
}

#[test]
fn every_lake_source_is_classified_into_exactly_one_zone() {
    let lake = package_dir("dumpster-kit-lake");
    let mut actual: Vec<String> = rust_sources(&lake)
        .iter()
        .map(|p| {
            p.strip_prefix(lake.join("src"))
                .unwrap()
                .display()
                .to_string()
        })
        .collect();
    actual.sort();
    let mut classified: Vec<String> = [CATALOG_ZONE_RS, TENANT_ZONE_RS, ZONE_ROOT_RS]
        .concat()
        .iter()
        .map(|s| s.to_string())
        .collect();
    classified.sort();
    assert_eq!(
        actual, classified,
        "a lake-crate source is in no zone (or in two): classify it in this file"
    );
}

#[test]
fn the_scanner_sees_what_it_claims_to() {
    let src = "// tenant in a comment\nlet a = \"tenant\"; /* tenant */ let b = '\"'; let c = r#\"raw/x\"#;\nlet tenant_id = 1;\n";
    let code = strip(src, false);
    assert_eq!(code.matches("tenant").count(), 1, "{code}");
    let kept = strip(src, true);
    assert!(
        kept.contains("\"tenant\"") && kept.contains("raw/x"),
        "{kept}"
    );

    // Seen red: a planted violation is found, a tenant-keyed addition is not.
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("catalog.rs");
    std::fs::write(
        &f,
        "fn f(db: &str) { open_tenant_zone(); let _ = \"tenants\"; }\n",
    )
    .unwrap();
    assert_eq!(scan(&["tenant"], std::slice::from_ref(&f), false).len(), 1);
    std::fs::write(&f, "// the tenant is out of scope\nfn f() {}\n").unwrap();
    assert!(scan(&["tenant"], std::slice::from_ref(&f), false).is_empty());
}
