//! Search query language metadata tables (decision D1/D2).
//!
//! Keyword definitions, the rarity rank table, and `is:`/`has:` flag
//! definitions are DATA, not code. The authoring sources are
//! `data/search_keywords.json`, `data/rarities.json`, and
//! `data/search_flags.json`; [`reconcile`] re-applies them into the
//! `search_keywords` / `rarities` / `search_flags` tables on every
//! `dumpstar setup` and every offline `dumpstar-lake-derive shared` — and on
//! every `dumpstar serve` startup, which is what keeps a fresh deploy from
//! serving an empty keyword registry.
//!
//! The pure parser ([`dumpstar_core::query`]) borrows the [`KeywordRegistry`]
//! that [`load_registry`] builds from the table; the SQL compiler
//! (`search.rs`, task idf.7) reads the rarity ranks and flag definitions.

use dumpstar_core::query::{KeywordDef, KeywordRegistry};
use rusqlite::{Connection, params};

use crate::error::Result;

pub(crate) const KEYWORDS_SEED: &str = include_str!("../../../data/search_keywords.json");

/// The search vocabulary file, verbatim: what the agent tool description and `query_help` are
/// generated from, so they cannot drift from the UI's own help.
pub fn keywords_json() -> &'static str {
    KEYWORDS_SEED
}
// `rarities.json` ranks the Star Wars: Unlimited tiers (Common through
// Legendary, Special, and Promo) so `r>=` / `r<` compare by rank.
pub(crate) const RARITIES_SEED: &str = include_str!("../../../data/rarities.json");
pub(crate) const FLAGS_SEED: &str = include_str!("../../../data/search_flags.json");

/// One row of the `rarities` rank table.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Rarity {
    pub name: String,
    #[serde(rename = "rank")]
    pub rank: i64,
    /// Group alias (`secret`, `ultra`, `common`…), if any.
    #[serde(default)]
    pub grp: Option<String>,
}

/// One rarity's official mark, as the catalog apply stored it in
/// `rarity_marks`: the one-letter character, its colour and the publisher's
/// sort position.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct RarityMark {
    /// Official rarity name, e.g. `Legendary`; the accessible label.
    pub name: String,
    /// The official one-letter character, e.g. `L`.
    pub character: String,
    /// `#rrggbb`.
    pub color: String,
    #[ts(type = "number")]
    pub sort_value: i64,
}

/// Every stored rarity mark, in the publisher's sort order.
pub fn load_rarity_marks(conn: &Connection) -> Result<Vec<RarityMark>> {
    let mut stmt = conn.prepare(
        "SELECT name, character, color, sort_value FROM rarity_marks ORDER BY sort_value, name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(RarityMark {
            name: r.get(0)?,
            character: r.get(1)?,
            color: r.get(2)?,
            sort_value: r.get(3)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One row of the `search_flags` table — an `is:` flag definition.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SearchFlag {
    pub flag: String,
    /// `variant_match` (match `match_str` against the printing's variant), `finish` (match it against the finish) or
    /// `computed` (a named predicate the compiler implements).
    pub kind: String,
    #[serde(rename = "match", default)]
    pub match_str: Option<String>,
    #[serde(default)]
    pub predicate: Option<String>,
    #[serde(default)]
    pub help: Option<String>,
}

/// Counts written by [`reconcile`].
#[derive(Debug, Clone, Copy)]
pub struct SearchMetaCounts {
    pub keywords: usize,
    pub rarities: usize,
    pub flags: usize,
}

/// What the three search-metadata tables currently hold.
///
/// The counts [`reconcile`] would report, read back rather than produced by
/// re-running it. Opening the catalog read-write already reconciles these
/// (`connection.rs::converge`), so a caller that wants to *say* how many
/// there are asks — a second reconcile for the sake of its return value is a
/// few hundred rows rewritten to print a number that was already true.
pub fn counts(conn: &Connection) -> Result<SearchMetaCounts> {
    let one = |sql: &str| -> Result<usize> {
        Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))? as usize)
    };
    Ok(SearchMetaCounts {
        keywords: one("SELECT COUNT(*) FROM search_keywords")?,
        rarities: one("SELECT COUNT(*) FROM rarities")?,
        flags: one("SELECT COUNT(*) FROM search_flags")?,
    })
}

/// Re-seed the three search-metadata tables from their JSON sources. These
/// tables hold no dynamic rows and nothing references them, so each is fully
/// replaced — idempotent by construction.
pub fn reconcile(conn: &mut Connection) -> Result<SearchMetaCounts> {
    let keywords: Vec<KeywordDef> = serde_json::from_str(KEYWORDS_SEED)?;
    let rarities: Vec<Rarity> = serde_json::from_str(RARITIES_SEED)?;
    let flags: Vec<SearchFlag> = serde_json::from_str(FLAGS_SEED)?;

    let tx = conn.transaction()?;

    tx.execute("DELETE FROM search_keywords", [])?;
    for k in &keywords {
        let aliases = serde_json::to_string(&k.aliases)?;
        let operators = serde_json::to_string(&k.operators)?;
        tx.execute(
            "INSERT INTO search_keywords
               (canonical, aliases, operators, kind, target, value_enum, semantics, help)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                k.canonical,
                aliases,
                operators,
                k.kind,
                k.target,
                k.value_enum,
                k.semantics,
                k.help,
            ],
        )?;
    }

    tx.execute("DELETE FROM rarities", [])?;
    for r in &rarities {
        tx.execute(
            "INSERT INTO rarities (name, rank, grp) VALUES (?1, ?2, ?3)",
            params![r.name, r.rank, r.grp],
        )?;
    }

    tx.execute("DELETE FROM search_flags", [])?;
    for f in &flags {
        tx.execute(
            "INSERT INTO search_flags (flag, kind, match_str, predicate, help)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![f.flag, f.kind, f.match_str, f.predicate, f.help],
        )?;
    }

    tx.commit()?;

    Ok(SearchMetaCounts {
        keywords: keywords.len(),
        rarities: rarities.len(),
        flags: flags.len(),
    })
}

/// Build the parser's [`KeywordRegistry`] from the `search_keywords` table.
/// Call once at server startup; the parser borrows the result.
pub fn load_registry(conn: &Connection) -> Result<KeywordRegistry> {
    type Row = (
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let mut stmt = conn.prepare(
        "SELECT canonical, aliases, operators, kind, target, value_enum, semantics, help
         FROM search_keywords ORDER BY canonical",
    )?;
    let rows: Vec<Row> = stmt
        .query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let mut defs = Vec::with_capacity(rows.len());
    for (canonical, aliases, operators, kind, target, value_enum, semantics, help) in rows {
        defs.push(KeywordDef {
            canonical,
            aliases: serde_json::from_str(&aliases)?,
            operators: serde_json::from_str(&operators)?,
            kind,
            target,
            value_enum,
            semantics,
            help,
        });
    }
    Ok(KeywordRegistry::new(defs))
}

/// Load the rarity rank table.
pub fn load_rarities(conn: &Connection) -> Result<Vec<Rarity>> {
    let mut stmt = conn.prepare("SELECT name, rank, grp FROM rarities ORDER BY rank, name")?;
    let rows = stmt.query_map([], |r| {
        Ok(Rarity {
            name: r.get(0)?,
            rank: r.get(1)?,
            grp: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Resolve raw catalog rarity strings against the `rarities` table.
///
/// Upstream is inconsistent about spelling: the official API ships
/// `"Legendary"` while TCGCSV tiers can arrive as `"LEGENDARY"`. Both name
/// the same tier, so the lookup key is a
/// normalised form (lowercased, underscores as spaces, whitespace
/// collapsed) and the table's own `name` is the canonical spelling to
/// display.
///
/// This is the one place that normalisation lives. Before it existed, the
/// stats page carried its own `canonicalRarity()` plus a `RARITY_ORDER`
/// map in TypeScript — a second, drifting copy of a typology the shared
/// catalog already owns.
pub struct RarityLookup {
    by_key: std::collections::HashMap<String, Rarity>,
    sort_values: std::collections::HashMap<String, i64>,
}

impl RarityLookup {
    /// Build the lookup from the `rarities` table.
    pub fn load(conn: &Connection) -> Result<Self> {
        let by_key = load_rarities(conn)?
            .into_iter()
            .map(|r| (normalize_rarity(&r.name), r))
            .collect();
        let sort_values = load_rarity_marks(conn)?
            .into_iter()
            .map(|m| (normalize_rarity(&m.name), m.sort_value))
            .collect();
        Ok(Self {
            by_key,
            sort_values,
        })
    }

    /// The table row for a raw catalog rarity, if the tier is ranked.
    pub fn get(&self, raw: &str) -> Option<&Rarity> {
        self.by_key.get(&normalize_rarity(raw))
    }

    /// Canonical display spelling — the table's `name`, or the raw string
    /// unchanged when the tier is not in the table. An unranked tier is a
    /// gap in `data/rarities.json`, not a reason to hide the card.
    pub fn display(&self, raw: &str) -> String {
        self.get(raw)
            .map_or_else(|| raw.to_string(), |r| r.name.clone())
    }

    /// Curated ordinal, or [`UNRANKED_RARITY`] for a tier the table does
    /// not carry — which sorts every unknown tier after the known ones.
    pub fn rank(&self, raw: &str) -> i64 {
        self.get(raw).map_or(UNRANKED_RARITY, |r| r.rank)
    }

    /// The publisher's sort value for a rarity (`rarity_marks`), or
    /// [`UNRANKED_RARITY`] for one the catalog gave no mark.
    pub fn sort_value(&self, raw: &str) -> i64 {
        self.sort_values
            .get(&normalize_rarity(raw))
            .copied()
            .unwrap_or(UNRANKED_RARITY)
    }

    /// Group alias (`common`, `rare`, `legendary`…) — the key the stats page
    /// colours its histogram by.
    pub fn grp(&self, raw: &str) -> Option<String> {
        self.get(raw).and_then(|r| r.grp.clone())
    }
}

/// Rank given to a rarity string absent from the `rarities` table.
pub const UNRANKED_RARITY: i64 = 1000;

/// Fold a rarity string into its lookup key: `"MEGA_ATTACK_RARE"` and
/// `"Mega Attack Rare"` both become `"mega attack rare"`.
fn normalize_rarity(raw: &str) -> String {
    raw.split(|c: char| c == '_' || c.is_whitespace())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Load the `is:`/`has:` flag definitions.
pub fn load_flags(conn: &Connection) -> Result<Vec<SearchFlag>> {
    let mut stmt = conn
        .prepare("SELECT flag, kind, match_str, predicate, help FROM search_flags ORDER BY flag")?;
    let rows = stmt.query_map([], |r| {
        Ok(SearchFlag {
            flag: r.get(0)?,
            kind: r.get(1)?,
            match_str: r.get(2)?,
            predicate: r.get(3)?,
            help: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open_shared;

    fn shared() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = open_shared(&dir.path().join("shared.sqlite")).unwrap();
        (dir, conn)
    }

    #[test]
    fn seeds_parse_and_have_unique_aliases() {
        // The embedded JSON must parse, and no alias may map to two keywords
        // (a HashMap would silently clobber the duplicate).
        let keywords: Vec<KeywordDef> = serde_json::from_str(KEYWORDS_SEED).unwrap();
        assert!(keywords.len() >= 35, "expected the full keyword map");
        let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        for k in &keywords {
            for a in std::iter::once(&k.canonical).chain(k.aliases.iter()) {
                let lower = a.to_ascii_lowercase();
                if let Some(prev) = seen.insert(lower.clone(), k.canonical.clone()) {
                    // An alias repeating its own canonical is harmless; only a
                    // collision across two different keywords is a bug.
                    assert_eq!(
                        prev, k.canonical,
                        "alias '{lower}' maps to both '{prev}' and '{}'",
                        k.canonical
                    );
                }
            }
        }
        let _: Vec<Rarity> = serde_json::from_str(RARITIES_SEED).unwrap();
        let _: Vec<SearchFlag> = serde_json::from_str(FLAGS_SEED).unwrap();
    }

    #[test]
    fn reconcile_is_idempotent() {
        let (_d, mut conn) = shared();
        let a = reconcile(&mut conn).unwrap();
        let b = reconcile(&mut conn).unwrap();
        assert_eq!(a.keywords, b.keywords);
        assert_eq!(a.rarities, b.rarities);
        assert_eq!(a.flags, b.flags);
        let n: i64 = conn
            .query_row("SELECT count(*) FROM search_keywords", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n as usize, b.keywords);
    }

    #[test]
    fn load_registry_resolves_aliases() {
        let (_d, mut conn) = shared();
        reconcile(&mut conn).unwrap();
        let reg = load_registry(&conn).unwrap();
        assert!(!reg.is_empty());
        assert_eq!(reg.resolve("t"), Some("card_type"));
        assert_eq!(reg.resolve("TYPE"), Some("card_type"));
        assert_eq!(reg.resolve("a"), Some("aspect"));
        assert_eq!(reg.resolve("c"), Some("cost"));
        assert_eq!(reg.resolve("s"), Some("set"));
        assert_eq!(reg.resolve("is"), Some("is_flag"));
        assert_eq!(reg.resolve("qty"), Some("qty"));
        assert_eq!(reg.resolve("bogus"), None);
        // The Pokémon vocabulary is gone, not merely unused.
        assert_eq!(reg.resolve("dex"), None);
        assert_eq!(reg.resolve("energy"), None);
    }

    #[test]
    fn rarities_are_ordered_low_to_high() {
        let (_d, mut conn) = shared();
        reconcile(&mut conn).unwrap();
        let rs = load_rarities(&conn).unwrap();
        let rank = |name: &str| rs.iter().find(|r| r.name == name).map(|r| r.rank);
        assert!(rank("Common").unwrap() < rank("Uncommon").unwrap());
        assert!(rank("Uncommon").unwrap() < rank("Rare").unwrap());
        assert!(rank("Rare").unwrap() < rank("Legendary").unwrap());
        assert!(rank("Legendary").unwrap() < rank("Special").unwrap());
        assert_eq!(
            rs.iter()
                .find(|r| r.name == "Legendary")
                .unwrap()
                .grp
                .as_deref(),
            Some("legendary")
        );
    }

    #[test]
    fn rarity_lookup_folds_upstream_spelling_variants() {
        let (_d, mut conn) = shared();
        reconcile(&mut conn).unwrap();
        let tiers = RarityLookup::load(&conn).unwrap();

        // The official API's title case and TCGCSV's SCREAMING_SNAKE name the
        // same tier; both resolve to the seed's spelling.
        for raw in ["Legendary", "LEGENDARY", "legendary", "  legendary "] {
            assert_eq!(tiers.display(raw), "Legendary", "raw = {raw}");
            assert_eq!(tiers.grp(raw).as_deref(), Some("legendary"), "raw = {raw}");
        }
        assert_eq!(tiers.grp("Special").as_deref(), Some("special"));
        assert!(tiers.rank("Common") < tiers.rank("Legendary"));

        // A tier the seed does not carry keeps its string and sorts last.
        assert_eq!(tiers.display("Blorbo Rare"), "Blorbo Rare");
        assert_eq!(tiers.rank("Blorbo Rare"), UNRANKED_RARITY);
        assert_eq!(tiers.grp("Blorbo Rare"), None);
        assert!(tiers.rank("Blorbo Rare") > tiers.rank("Special"));
    }

    #[test]
    fn flags_have_computed_and_variant_kinds() {
        let (_d, mut conn) = shared();
        reconcile(&mut conn).unwrap();
        let flags = load_flags(&conn).unwrap();
        let missing = flags.iter().find(|f| f.flag == "missing").unwrap();
        assert_eq!(missing.kind, "computed");
        assert_eq!(missing.predicate.as_deref(), Some("missing"));
        let hyperspace = flags.iter().find(|f| f.flag == "hyperspace").unwrap();
        assert_eq!(hyperspace.kind, "variant_match");
        assert_eq!(hyperspace.match_str.as_deref(), Some("Hyperspace"));
        let foil = flags.iter().find(|f| f.flag == "foil").unwrap();
        assert_eq!(foil.kind, "finish");
        assert_eq!(foil.match_str.as_deref(), Some("Foil"));
    }

    #[test]
    fn rarity_sort_follows_the_stored_official_sort_value() {
        let (_d, mut conn) = shared();
        reconcile(&mut conn).unwrap();
        for (n, c, col, v) in [
            ("Legendary", "L", "#00c8f3", 5),
            ("Special", "S", "#000000", 1),
        ] {
            conn.execute(
                "INSERT INTO rarity_marks (name, character, color, sort_value) VALUES (?1,?2,?3,?4)",
                params![n, c, col, v],
            )
            .unwrap();
        }
        let marks = load_rarity_marks(&conn).unwrap();
        assert_eq!(marks[0].name, "Special");
        assert_eq!(marks[1].character, "L");
        let tiers = RarityLookup::load(&conn).unwrap();
        // The curated search rank puts Special above Legendary; the official
        // sort value does not.
        assert!(tiers.sort_value("Special") < tiers.sort_value("LEGENDARY"));
        assert_eq!(tiers.sort_value("Mythic"), UNRANKED_RARITY);
    }
}
