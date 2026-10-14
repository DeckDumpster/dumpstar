//! Dumpstar's lake, key custody, shipper, eraser and derive parts are the kit's. This is where
//! that is held to:
//!
//! * the names the kit's environment types derive from `DUMPSTAR` are the names the deploy
//!   units and `~/.config/dumpstar/` already use;
//! * the zone read-back's source map is held to the outbox's, in both directions (the gate
//!   that stopped a holding source going undeclared);
//! * a guard that the copies this repository was forked with stay deleted.

use std::path::{Path, PathBuf};

#[test]
fn the_lake_reads_the_names_the_deploy_units_already_use() {
    let env = dumpstar_ship::lake_env();
    assert_eq!(env.bucket, "DUMPSTAR_LAKE_S3_BUCKET");
    assert_eq!(env.dir, "DUMPSTAR_LAKE_DIR");
    assert_eq!(env.env_file, "DUMPSTAR_LAKE_ENV");
    assert_eq!(env.config_relative_path, ".config/dumpstar/lake.env");
}

#[test]
fn key_custody_reads_the_names_and_format_the_existing_master_key_uses() {
    let env = dumpstar_ship::key_env();
    assert_eq!(env.key_file_var, "DUMPSTAR_MASTER_KEY_FILE");
    assert_eq!(
        env.default_relative_path,
        ".config/dumpstar/tenant-master.key"
    );
    assert_eq!(env.format_tag(), "dumpstar-master-key-v1:");
}

#[test]
fn every_outbox_source_has_a_staging_table() {
    use dumpstar_db::outbox::{SOURCE_TABLES, ZONE_SOURCES};
    for (source, _) in SOURCE_TABLES {
        assert!(
            ZONE_SOURCES.iter().any(|(table, _)| table == source),
            "the outbox ships `{source}` and the zone reader has no staging table for it"
        );
    }
    for (source, _) in ZONE_SOURCES {
        assert!(
            SOURCE_TABLES.iter().any(|(table, _)| table == source),
            "the zone reader materialises `{source}`, which the outbox never ships"
        );
    }
}

fn rust_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_under(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

/// These are the fingerprints of the copies this repository was forked with; none may
/// reappear under `crates/`.
#[test]
fn the_forked_lakehouse_copies_are_gone() {
    const COPIES: &[(&str, &str)] = &[
        ("struct RawLanding", "the raw landing sink"),
        ("trait ObjectStore", "the object store seam"),
        ("struct DeriveClock", "the derive clock"),
        ("fn select_run", "run selection"),
        ("struct RawReplay", "partition replay"),
        ("Hkdf::<Sha256>", "per-tenant key derivation"),
        ("Aes256Gcm", "the tenant-zone envelope"),
        ("fn finalize_landing", "the landing manifest bracket"),
        ("pub fn project(", "the outbox reduction"),
        ("fn compare(", "the catalog comparator"),
    ];
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    rust_under(&crates, &mut files);
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
        "lakehouse code lives in dumpster-kit-*, not here:\n{}",
        found.join("\n")
    );
}
