//! The remedial set-art asset set: each set's official logo and an art crop
//! (the illustration without the card frame), kept in the catalog's own store
//! and served by the site itself. Nothing is hot-linked from the publisher's
//! CDN at page load.
//!
//! `data/set_art.json` lists, for every set, where its logo and crop come from
//! (or an explicit `none` with the reason). [`fetch`] downloads each listed
//! asset once into `<art_dir>/` and records its source URL, retrieval date and
//! SHA-256 in `<art_dir>/manifest.json`. [`resolve`] answers from files that
//! exist, so a set with nothing retrieved falls back to its name.

use std::collections::BTreeMap;
use std::path::Path;

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{CatalogError, Result};
use crate::fetch::Fetch;

/// The list of assets to retrieve, compiled in.
pub const SOURCES: &str = include_str!("../data/set_art.json");
/// The URL prefix the site serves `<art_dir>` under.
pub const URL_PREFIX: &str = "/art";
const MANIFEST: &str = "manifest.json";

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    pub source_url: String,
    pub file: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetSources {
    pub set_code: String,
    pub logo: Option<Source>,
    pub crop: Option<Source>,
    /// Why the set has no logo or crop; required when both are absent.
    pub none: Option<String>,
    /// Why the set has no crop; required when `crop` is absent.
    #[serde(default)]
    pub crop_none: Option<String>,
}

#[derive(Deserialize)]
struct SourceFile {
    sets: Vec<SetSources>,
}

/// The compiled-in source list.
pub fn sources() -> Vec<SetSources> {
    serde_json::from_str::<SourceFile>(SOURCES)
        .expect("data/set_art.json parses")
        .sets
}

/// One retrieved asset, as recorded in `<art_dir>/manifest.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Retrieved {
    pub file: String,
    pub source_url: String,
    pub retrieved_on: NaiveDate,
    pub sha256: String,
}

fn read_manifest(art_dir: &Path) -> Result<BTreeMap<String, Retrieved>> {
    match std::fs::read(art_dir.join(MANIFEST)) {
        Ok(b) => serde_json::from_slice(&b).map_err(Into::into),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(CatalogError::Source(format!("{MANIFEST}: {e}"))),
    }
}

/// What [`fetch`] did.
#[derive(Debug, Default, PartialEq)]
pub struct FetchReport {
    pub fetched: usize,
    pub already: usize,
    pub failed: Vec<String>,
}

/// Retrieve every listed asset not yet in the store. An asset already
/// retrieved from the same URL is left alone, so a refresh is cheap and the
/// retrieval date stays the day the bytes were first taken.
pub fn fetch(
    art_dir: &Path,
    http: &dyn Fetch,
    today: NaiveDate,
    list: &[SetSources],
) -> Result<FetchReport> {
    std::fs::create_dir_all(art_dir)
        .map_err(|e| CatalogError::Source(format!("{}: {e}", art_dir.display())))?;
    let mut manifest = read_manifest(art_dir)?;
    let mut report = FetchReport::default();
    for set in list {
        for (kind, src) in [("logo", &set.logo), ("crop", &set.crop)] {
            let Some(src) = src else { continue };
            let key = format!("{}/{kind}", set.set_code);
            let have = manifest.get(&key);
            if have.is_some_and(|r| r.source_url == src.source_url)
                && art_dir.join(&src.file).is_file()
            {
                report.already += 1;
                continue;
            }
            match http.get(&src.source_url) {
                Ok(bytes) if !bytes.is_empty() => {
                    std::fs::write(art_dir.join(&src.file), &bytes)
                        .map_err(|e| CatalogError::Source(format!("{}: {e}", src.file)))?;
                    manifest.insert(
                        key,
                        Retrieved {
                            file: src.file.clone(),
                            source_url: src.source_url.clone(),
                            retrieved_on: today,
                            sha256: hex::encode(Sha256::digest(&bytes)),
                        },
                    );
                    report.fetched += 1;
                }
                Ok(_) => report.failed.push(format!("{key}: empty body")),
                Err(e) => report.failed.push(format!("{key}: {e}")),
            }
        }
    }
    std::fs::write(
        art_dir.join(MANIFEST),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .map_err(|e| CatalogError::Source(format!("{MANIFEST}: {e}")))?;
    Ok(report)
}

/// Every listed asset's source URL, `(<set>/<kind>, url)`: what the offline
/// landing unit fetches into the lake (the derive replays them through
/// [`fetch`]).
pub fn urls(list: &[SetSources]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for set in list {
        for (kind, src) in [("logo", &set.logo), ("crop", &set.crop)] {
            if let Some(src) = src {
                out.push((format!("{}/{kind}", set.set_code), src.source_url.clone()));
            }
        }
    }
    out
}

/// What a set-art store holds against what the list promises.
#[derive(Debug, Default, PartialEq)]
pub struct Coverage {
    /// Sets the list gives at least one asset.
    pub expected: usize,
    /// Of those, the sets with at least one asset on disk.
    pub have: usize,
    /// Listed assets (`<set>/<kind>`) not in the store.
    pub missing: Vec<String>,
}

impl Coverage {
    pub fn line(&self) -> String {
        format!("set art: {}/{} sets", self.have, self.expected)
    }
}

/// Count the sets of `list` with art in `art_dir`, and name every listed asset
/// that is absent. A set reviewed as having none is not expected to have any.
pub fn coverage(art_dir: &Path, list: &[SetSources]) -> Coverage {
    let mut c = Coverage::default();
    for set in list {
        let art = resolve(art_dir, &set.set_code);
        let wanted = [
            ("logo", &set.logo, &art.logo_url),
            ("crop", &set.crop, &art.crop_url),
        ];
        if wanted.iter().all(|(_, s, _)| s.is_none()) {
            continue;
        }
        c.expected += 1;
        if art.logo_url.is_some() || art.crop_url.is_some() {
            c.have += 1;
        }
        for (kind, src, got) in wanted {
            if src.is_some() && got.is_none() {
                c.missing.push(format!("{}/{kind}", set.set_code));
            }
        }
    }
    c
}

/// A set's served art; each field is `None` when nothing was retrieved.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SetArt {
    pub logo_url: Option<String>,
    pub crop_url: Option<String>,
}

/// The main set a Weekly Play set shares its art with: its code minus the `P`
/// suffix (`LAWP` is `LAW`'s), for any set the category mapping files under
/// Weekly Play Promos. A new weekly-play set inherits with no list to edit.
pub fn parent_set(set_code: &str) -> Option<&str> {
    if crate::set_categories::category_for(set_code)? != "Weekly Play Promos" {
        return None;
    }
    set_code.strip_suffix('P')
}

/// The served art of `set_code`: a Weekly Play set shows its parent's logo and
/// crop; when the parent has none it falls back to its own.
pub fn resolve(art_dir: &Path, set_code: &str) -> SetArt {
    if let Some(parent) = parent_set(set_code) {
        let art = resolve_own(art_dir, parent);
        if art != SetArt::default() {
            return art;
        }
    }
    resolve_own(art_dir, set_code)
}

/// URLs under [`URL_PREFIX`] for the assets listed under exactly `set_code`
/// that exist in the store. A missing or unreadable store is no art, never an
/// error.
fn resolve_own(art_dir: &Path, set_code: &str) -> SetArt {
    let manifest = read_manifest(art_dir).unwrap_or_default();
    let url = |kind: &str| {
        let r = manifest.get(&format!("{set_code}/{kind}"))?;
        art_dir
            .join(&r.file)
            .is_file()
            .then(|| format!("{URL_PREFIX}/{}", r.file))
    };
    SetArt {
        logo_url: url("logo"),
        crop_url: url("crop"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake(Cell<usize>);
    impl Fetch for Fake {
        fn get(&self, url: &str) -> Result<Vec<u8>> {
            self.0.set(self.0.get() + 1);
            if url.contains("down") {
                Err(CatalogError::Source("HTTP 403".into()))
            } else {
                Ok(b"png".to_vec())
            }
        }
    }

    fn day() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 15).unwrap()
    }

    fn list() -> Vec<SetSources> {
        serde_json::from_str::<SourceFile>(
            r#"{"sets":[
              {"set_code":"AAA","logo":{"source_url":"https://x/a.png","file":"AAA-logo.png"},
               "crop":{"source_url":"https://x/down.png","file":"AAA-crop.png"}}]}"#,
        )
        .unwrap()
        .sets
    }

    #[test]
    fn fetch_records_provenance_and_resolve_serves_only_what_exists() {
        let dir = tempfile::tempdir().unwrap();
        let http = Fake(Cell::new(0));
        let r = fetch(dir.path(), &http, day(), &list()).unwrap();
        assert_eq!((r.fetched, r.failed.len()), (1, 1));
        let art = resolve(dir.path(), "AAA");
        assert_eq!(art.logo_url.as_deref(), Some("/art/AAA-logo.png"));
        assert_eq!(art.crop_url, None);
        assert_eq!(resolve(dir.path(), "ZZZ"), SetArt::default());
        let m = read_manifest(dir.path()).unwrap();
        assert_eq!(m["AAA/logo"].retrieved_on, day());
        assert_eq!(m["AAA/logo"].source_url, "https://x/a.png");
        // A second run does not re-take the logo.
        let again = fetch(dir.path(), &http, day(), &list()).unwrap();
        assert_eq!((again.fetched, again.already), (0, 1));
    }

    #[test]
    fn coverage_reports_a_set_whose_art_is_not_in_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let before = coverage(dir.path(), &list());
        assert_eq!((before.expected, before.have), (1, 0));
        assert_eq!(before.missing, ["AAA/logo", "AAA/crop"]);
        fetch(dir.path(), &Fake(Cell::new(0)), day(), &list()).unwrap();
        let after = coverage(dir.path(), &list());
        assert_eq!(
            (after.have, after.missing.clone()),
            (1, vec!["AAA/crop".into()])
        );
        assert_eq!(after.line(), "set art: 1/1 sets");
    }

    #[test]
    fn weekly_play_sets_resolve_to_their_parents_art() {
        let dir = tempfile::tempdir().unwrap();
        let all = sources();
        fetch(dir.path(), &Fake(Cell::new(0)), day(), &all).unwrap();
        assert_eq!(parent_set("LAWP"), Some("LAW"));
        assert_eq!(parent_set("LAW"), None);
        let law = resolve(dir.path(), "LAW");
        assert!(law.logo_url.is_some() && law.crop_url.is_some());
        assert_eq!(resolve(dir.path(), "LAWP"), law);
        let weekly: Vec<&str> = include_str!("../../../data/remedial/set-categories.csv")
            .lines()
            .filter(|l| l.split(',').nth(1) == Some("Weekly Play Promos"))
            .filter_map(|l| l.split(',').next())
            .collect();
        assert!(weekly.len() >= 6);
        for code in weekly {
            let parent = parent_set(code).expect(code);
            assert_eq!(
                resolve(dir.path(), code),
                resolve(dir.path(), parent),
                "{code}"
            );
        }
    }

    #[test]
    fn no_store_is_no_art() {
        assert_eq!(
            resolve(Path::new("/nonexistent/art"), "SOR"),
            SetArt::default()
        );
    }
}
