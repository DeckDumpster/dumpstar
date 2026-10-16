//! Every catalog set has a set-art manifest entry — a logo/crop, or an explicit
//! `none` with its reason — so a new set cannot reach the site unaccounted for.

use std::collections::BTreeSet;
use std::path::Path;

use dumpstar_catalog::art;

fn manifest_codes() -> BTreeSet<String> {
    art::sources().into_iter().map(|s| s.set_code).collect()
}

#[test]
fn every_tcgcsv_group_and_fake_set_has_an_entry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/swu/swu-groups.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(root).unwrap()).unwrap();
    let have = manifest_codes();
    let mut missing = vec![];
    for g in v["results"].as_array().unwrap() {
        let a = g["abbreviation"].as_str().unwrap_or("");
        if !a.is_empty() && !have.contains(a) {
            missing.push(a.to_string());
        }
    }
    for code in ["SOR", "SHD"] {
        if !have.contains(code) {
            missing.push(code.into());
        }
    }
    assert!(
        missing.is_empty(),
        "sets with no set_art.json entry: {missing:?}"
    );
}

#[test]
fn an_entry_is_art_or_says_why_not() {
    for s in art::sources() {
        assert!(
            s.logo.is_some()
                || s.crop.is_some()
                || s.none.as_deref().is_some_and(|n| !n.is_empty()),
            "{}: no logo, no crop and no `none` reason",
            s.set_code
        );
    }
}

/// Sets with no official logo, each reviewed with a reason in `set_art.json`.
/// This list only shrinks: a set that gains a logo leaves it, and a new set
/// without one needs a reviewed reason and an entry here.
const NO_LOGO_REVIEWED: &[&str] = &[
    "P26", "C26", "GFT", "CE2025", "SN1", "CE2024", "EEP", "JDG", "OPP", "PRE",
];

#[test]
fn a_set_without_a_logo_is_a_reviewed_exception() {
    let mut unreviewed = vec![];
    for s in art::sources() {
        if s.logo.is_some() {
            assert!(
                !NO_LOGO_REVIEWED.contains(&s.set_code.as_str()),
                "{} has a logo now: drop it from NO_LOGO_REVIEWED",
                s.set_code
            );
            continue;
        }
        let reason = s.none.as_deref().unwrap_or("");
        if !NO_LOGO_REVIEWED.contains(&s.set_code.as_str()) || reason.contains("recorded yet") {
            unreviewed.push(s.set_code);
        }
    }
    assert!(
        unreviewed.is_empty(),
        "sets with no logo and no reviewed reason: {unreviewed:?}"
    );
}

#[test]
fn every_logo_names_its_official_source() {
    for s in art::sources() {
        if let Some(l) = s.logo {
            assert!(
                l.source_url
                    .starts_with("https://cdn.starwarsunlimited.com/"),
                "{}: logo is not from the official CDN: {}",
                s.set_code,
                l.source_url
            );
            assert!(
                l.file.starts_with(&s.set_code),
                "{}: file {}",
                s.set_code,
                l.file
            );
        }
    }
}

#[test]
fn every_set_has_a_banner_crop_or_says_why_not() {
    for s in art::sources() {
        match s.crop {
            Some(c) => {
                assert!(
                    c.source_url
                        .starts_with("https://cdn.starwarsunlimited.com/"),
                    "{}: crop is not from the official CDN: {}",
                    s.set_code,
                    c.source_url
                );
                assert_eq!(c.file, format!("{}-crop.png", s.set_code));
            }
            None => assert!(
                s.crop_none.as_deref().is_some_and(|n| !n.is_empty()),
                "{}: no crop and no `crop_none` reason",
                s.set_code
            ),
        }
    }
}

#[test]
fn fetching_the_manifest_stores_a_crop_for_every_set_that_lists_one() {
    use chrono::NaiveDate;
    struct Ok_;
    impl dumpstar_catalog::fetch::Fetch for Ok_ {
        fn get(&self, _: &str) -> dumpstar_catalog::Result<Vec<u8>> {
            Ok(b"png".to_vec())
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let list = art::sources();
    let day = NaiveDate::from_ymd_opt(2026, 10, 16).unwrap();
    let r = art::fetch(dir.path(), &Ok_, day, &list).unwrap();
    assert!(r.failed.is_empty());
    for s in &list {
        assert_eq!(
            art::resolve(dir.path(), &s.set_code).crop_url.is_some(),
            s.crop.is_some(),
            "{}",
            s.set_code
        );
    }
}
