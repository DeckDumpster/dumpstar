//! CSV import: resolve `dumpstar-core`-parsed rows against the catalog into
//! a preview report, then commit the matched rows (PLAN.md §9).

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension};

use dumpstar_core::import::collectr;
use dumpstar_core::import::{self, ParsedRow};

use crate::batches::{self, NewBatch};
use crate::collection::{self, CopyEdit, NewCopy};
use crate::error::{DbError, Result};
use crate::sealed_import::{self, SealedCommitResult, SealedResolutionReport};

/// A supported import file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportFormat {
    Manabox,
    Tcgplayer,
    /// Dumpstar-native: set code, collector number, finish.
    Dumpstar,
    /// Collectr (getcollectr.com). Yields both singles and sealed products,
    /// so it goes through the combined path ([`preview_collectr`] /
    /// [`commit_collectr`]), never the singles-only [`parse`].
    Collectr,
}

impl ImportFormat {
    /// Parse the wire name (`"manabox"`, `"tcgplayer"`, `"dumpstar"`,
    /// `"collectr"`).
    pub fn parse(name: &str) -> Result<Self> {
        match name.trim().to_lowercase().as_str() {
            "manabox" => Ok(Self::Manabox),
            "tcgplayer" => Ok(Self::Tcgplayer),
            "dumpstar" => Ok(Self::Dumpstar),
            "collectr" => Ok(Self::Collectr),
            other => Err(DbError::Import(format!("unknown import format '{other}'"))),
        }
    }

    /// The `collection.source` value for copies imported in this format.
    fn source(self) -> &'static str {
        match self {
            Self::Manabox => "csv_manabox",
            Self::Tcgplayer => "csv_tcgplayer",
            Self::Dumpstar => "csv_dumpstar",
            Self::Collectr => "csv_collectr",
        }
    }

    /// The `batches.batch_type` value for this format's import batches.
    fn batch_type(self) -> &'static str {
        match self {
            Self::Manabox => "csv_manabox",
            Self::Tcgplayer => "csv_tcgplayer",
            Self::Dumpstar => "csv_dumpstar",
            Self::Collectr => "csv_collectr",
        }
    }
}

/// Parse CSV `content` in the given format into pre-resolution single-card
/// rows. Collectr is rejected here: it also yields sealed products, so it
/// must go through [`preview_collectr`] / [`commit_collectr`].
fn parse(format: ImportFormat, content: &str) -> Result<Vec<ParsedRow>> {
    Ok(match format {
        ImportFormat::Manabox => import::manabox::parse(content)?,
        ImportFormat::Tcgplayer => import::tcgplayer::parse(content)?,
        ImportFormat::Dumpstar => import::dumpstar::parse(content)?,
        ImportFormat::Collectr => {
            return Err(DbError::Import(
                "Collectr yields sealed products too; use the combined import path".into(),
            ));
        }
    })
}

/// A row that resolved cleanly to a catalog printing — ready to commit.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ResolvedRow {
    #[ts(type = "number")]
    pub source_line: u32,
    pub printing_id: String,
    pub card_name: String,
    pub set_code: String,
    pub number: String,
    /// The printing's variant (`Standard`, `Hyperspace`, …).
    pub variant: String,
    /// `Normal` or `Foil`.
    pub finish: String,
    pub condition: String,
    pub language: String,
    pub purchase_price: Option<f64>,
    pub acquired_at: Option<String>,
    pub tags: Vec<String>,
    /// How many copies of this printing are already `owned` in the collection
    /// (duplicate flag for the import preview). Filled by [`annotate_owned`]
    /// after resolution; `0` straight out of [`resolve`]. (dumpstar-oq3i.4)
    #[ts(type = "number")]
    pub already_owned: u32,
}

/// A row that could not be resolved, with a human-readable reason.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct UnmatchedRow {
    #[ts(type = "number")]
    pub source_line: u32,
    pub set_hint: String,
    pub number: String,
    pub finish: String,
    pub reason: String,
}

/// The outcome of resolving an import file — a preview shown before commit.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ResolutionReport {
    pub matched: Vec<ResolvedRow>,
    pub unmatched: Vec<UnmatchedRow>,
}

/// The outcome of committing an import.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CommitResult {
    #[ts(type = "number")]
    pub batch_id: i64,
    #[ts(type = "number")]
    pub added: u32,
    #[ts(type = "number")]
    pub skipped: u32,
}

/// Resolve a set hint to a catalog `set_code`: exact code, then set name (the
/// explicit name field, then the hint read as a name).
///
/// Shared with the sealed import pipeline ([`crate::sealed_import`]).
pub(crate) fn resolve_set(
    conn: &Connection,
    hint: &str,
    name: Option<&str>,
) -> Result<Option<String>> {
    let by = |sql: &str, value: &str| -> Result<Option<String>> {
        Ok(conn
            .prepare(sql)?
            .query_row([value], |r| r.get(0))
            .optional()?)
    };
    if let Some(c) = by(
        "SELECT set_code FROM sets WHERE set_code = ?1 COLLATE NOCASE",
        hint,
    )? {
        return Ok(Some(c));
    }
    if let Some(n) = name
        && let Some(c) = by(
            "SELECT set_code FROM sets WHERE name = ?1 COLLATE NOCASE",
            n,
        )?
    {
        return Ok(Some(c));
    }
    by(
        "SELECT set_code FROM sets WHERE name = ?1 COLLATE NOCASE",
        hint,
    )
}

/// Strip trailing parenthetical/bracketed descriptor groups an import
/// platform appends to a card name that the catalog name doesn't carry —
/// "Luke Skywalker (Prerelease Promo)" → "Luke Skywalker". Only *trailing*
/// groups are removed (repeatedly), so interior text is untouched; the catalog
/// side is never stripped.
fn strip_trailing_descriptors(name: &str) -> String {
    let mut s = name.trim();
    loop {
        let open = match s.chars().last() {
            Some(')') => '(',
            Some(']') => '[',
            _ => break,
        };
        match s.rfind(open) {
            Some(pos) => s = s[..pos].trim_end(),
            None => break,
        }
    }
    s.to_string()
}

/// One catalog printing as the resolver sees it.
#[derive(Clone)]
struct Candidate {
    printing_id: String,
    set_code: String,
    /// The collector number, unpadded.
    number: String,
    variant: String,
    finish: String,
    /// `Name`, or `Name - Subtitle` for a card that has one.
    display_name: String,
    /// Both spellings an import platform may use, folded for comparison.
    folded: [String; 2],
}

fn candidates(conn: &Connection, set_code: Option<&str>) -> Result<Vec<Candidate>> {
    let mut stmt = conn.prepare(
        "SELECT p.printing_id, p.set_code, p.number, p.variant, p.finish, c.name, c.subtitle \
           FROM printings p JOIN cards c ON c.card_id = p.card_id \
          WHERE (?1 IS NULL OR p.set_code = ?1) \
          ORDER BY p.printing_id",
    )?;
    let rows = stmt.query_map([set_code], |r| {
        let name: String = r.get(5)?;
        let subtitle: Option<String> = r.get(6)?;
        let display_name = match &subtitle {
            Some(s) if !s.is_empty() => format!("{name} - {s}"),
            _ => name.clone(),
        };
        let fold = crate::sealed_import::normalize_name;
        Ok(Candidate {
            printing_id: r.get(0)?,
            set_code: r.get(1)?,
            number: import::normalize_collector_number(&r.get::<_, String>(2)?),
            variant: r.get(3)?,
            finish: r.get(4)?,
            folded: [fold(&display_name), fold(&name)],
            display_name,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Resolve parsed rows against the catalog, partitioning them into matched
/// printings and unmatched rows with reasons.
///
/// A printing is named by (set, collector number, finish): Standard,
/// Hyperspace and Showcase print under different numbers, so the number
/// carries the variant. When the stated set does not resolve, or does not
/// carry the number, a row that names its card is searched across every set
/// by (folded name, number).
pub fn resolve(conn: &Connection, rows: &[ParsedRow]) -> Result<ResolutionReport> {
    let mut matched = Vec::new();
    let mut unmatched = Vec::new();
    // A CSV usually touches a handful of sets — cache the hint→code lookup
    // and each set's printings.
    let mut set_cache: HashMap<(String, Option<String>), Option<String>> = HashMap::new();
    let mut by_set: HashMap<String, Vec<Candidate>> = HashMap::new();
    let mut everything: Option<Vec<Candidate>> = None;

    for row in rows {
        let line = row.source_line as u32;
        let unmatched_row = |reason: String| UnmatchedRow {
            source_line: line,
            set_hint: row.set_hint.clone(),
            number: row.number.clone(),
            finish: row.finish.clone(),
            reason,
        };

        let key = (row.set_hint.clone(), row.set_name.clone());
        let set_code = match set_cache.get(&key) {
            Some(c) => c.clone(),
            None => {
                let c = resolve_set(conn, &row.set_hint, row.set_name.as_deref())?;
                set_cache.insert(key, c.clone());
                c
            }
        };

        // The catalog stores collector numbers unpadded ('90'); Collectr
        // zero-pads them ('090'). Normalize both to the same canonical form.
        let lookup_number = import::normalize_collector_number(&row.number);

        // The printings that carry this number: in the stated set when it
        // resolved, else (for a row that names its card) in any set.
        let mut pool: Vec<Candidate> = match &set_code {
            Some(sc) => {
                if !by_set.contains_key(sc) {
                    by_set.insert(sc.clone(), candidates(conn, Some(sc))?);
                }
                by_set[sc]
                    .iter()
                    .filter(|c| c.number == lookup_number)
                    .cloned()
                    .collect()
            }
            None => Vec::new(),
        };
        let mut global = false;
        if pool.is_empty()
            && let Some(want) = row.name.as_deref()
        {
            let want = crate::sealed_import::normalize_name(&strip_trailing_descriptors(want));
            if !want.is_empty() {
                if everything.is_none() {
                    everything = Some(candidates(conn, None)?);
                }
                pool = everything
                    .as_ref()
                    .expect("loaded above")
                    .iter()
                    .filter(|c| c.number == lookup_number && c.folded.contains(&want))
                    .cloned()
                    .collect();
                global = true;
            }
        }
        if pool.is_empty() {
            unmatched.push(unmatched_row(match (&set_code, &row.name) {
                (Some(sc), Some(want)) => format!(
                    "card #{lookup_number} not found in {sc}; no '{want}' #{lookup_number} in any set"
                ),
                (Some(sc), None) => format!("card #{lookup_number} not found in {sc}"),
                (None, Some(want)) => format!(
                    "no card '{want}' #{lookup_number} in any set (set '{}' unknown)",
                    row.set_hint
                ),
                (None, None) => format!("unknown set '{}'", row.set_hint),
            }));
            continue;
        }
        if global {
            let mut sets: Vec<&str> = pool.iter().map(|c| c.set_code.as_str()).collect();
            sets.sort_unstable();
            sets.dedup();
            if sets.len() > 1 {
                unmatched.push(unmatched_row(format!(
                    "'{}' #{lookup_number} is ambiguous: matches {}",
                    row.name.as_deref().unwrap_or(""),
                    sets.iter()
                        .map(|sc| format!("{sc} #{lookup_number}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
                continue;
            }
        }

        let Some(hit) = pool.iter().find(|c| c.finish == row.finish) else {
            unmatched.push(unmatched_row(format!(
                "finish '{}' not available for {}",
                row.finish, pool[0].display_name
            )));
            continue;
        };

        matched.push(ResolvedRow {
            source_line: line,
            printing_id: hit.printing_id.clone(),
            card_name: hit.display_name.clone(),
            set_code: hit.set_code.clone(),
            number: lookup_number,
            variant: hit.variant.clone(),
            finish: hit.finish.clone(),
            condition: row.condition.clone(),
            language: row.language.clone(),
            purchase_price: row.purchase_price,
            acquired_at: row.acquired_at.clone(),
            tags: row.tags.clone(),
            already_owned: 0, // filled by annotate_owned() (oq3i.4)
        });
    }
    Ok(ResolutionReport { matched, unmatched })
}

/// Annotate each matched single-card row with how many `owned` copies of the
/// same printing already sit in the collection, so the import preview can flag
/// duplicates the user may want to deselect. Purely additive: runs *after*
/// [`resolve`], mutating the report in place. (dumpstar-oq3i.4)
pub fn annotate_owned(conn: &Connection, report: &mut ResolutionReport) -> Result<()> {
    if report.matched.is_empty() {
        return Ok(());
    }
    let mut counts: HashMap<String, u32> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT printing_id, COUNT(*) FROM collection \
         WHERE status = 'owned' GROUP BY printing_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (printing_id, n) = row?;
        counts.insert(printing_id, n as u32);
    }
    for m in &mut report.matched {
        m.already_owned = counts.get(&m.printing_id).copied().unwrap_or(0);
    }
    Ok(())
}

/// Annotate each matched sealed row with how many `owned` units of the same
/// product already sit in `sealed_collection` (quantity summed). Mirror of
/// [`annotate_owned`] for the sealed half. (dumpstar-oq3i.4)
pub fn annotate_owned_sealed(conn: &Connection, report: &mut SealedResolutionReport) -> Result<()> {
    if report.matched.is_empty() {
        return Ok(());
    }
    let mut counts: HashMap<i64, u32> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT product_id, COALESCE(SUM(quantity), 0) FROM sealed_collection \
         WHERE status = 'owned' GROUP BY product_id",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (product_id, n) = row?;
        counts.insert(product_id, n as u32);
    }
    for m in &mut report.matched {
        m.already_owned = counts.get(&m.product_id).copied().unwrap_or(0);
    }
    Ok(())
}

/// Keep only the matched rows whose `source_line` is in `include`; drop all
/// unmatched. Used by the selected-commit path so the user's per-row choices
/// in the preview are honored server-side (after a fresh re-resolve).
fn filter_report(report: &ResolutionReport, include: &[u32]) -> ResolutionReport {
    let keep: HashSet<u32> = include.iter().copied().collect();
    ResolutionReport {
        matched: report
            .matched
            .iter()
            .filter(|r| keep.contains(&r.source_line))
            .cloned()
            .collect(),
        unmatched: Vec::new(),
    }
}

/// Sealed mirror of [`filter_report`].
fn filter_sealed(report: &SealedResolutionReport, include: &[u32]) -> SealedResolutionReport {
    let keep: HashSet<u32> = include.iter().copied().collect();
    SealedResolutionReport {
        matched: report
            .matched
            .iter()
            .filter(|r| keep.contains(&r.source_line))
            .cloned()
            .collect(),
        unmatched: Vec::new(),
    }
}

/// A JSON array literal for tag strings. Tags are simple identifiers
/// (`misprint`, `altered`) so no escaping is needed.
pub(crate) fn tags_json(tags: &[String]) -> String {
    let items: Vec<String> = tags.iter().map(|t| format!("\"{t}\"")).collect();
    format!("[{}]", items.join(","))
}

/// Preview an import: parse and resolve without writing anything.
pub fn preview(conn: &Connection, format: ImportFormat, content: &str) -> Result<ResolutionReport> {
    let rows = parse(format, content)?;
    let mut report = resolve(conn, &rows)?;
    annotate_owned(conn, &mut report)?;
    Ok(report)
}

/// Commit an import: parse, resolve, then add every matched row under a
/// fresh batch. Re-resolves server-side rather than trusting a preview.
pub fn commit(
    conn: &mut Connection,
    format: ImportFormat,
    content: &str,
    batch_name: Option<&str>,
) -> Result<CommitResult> {
    let rows = parse(format, content)?;
    let report = resolve(conn, &rows)?;
    commit_matched(
        conn,
        &report,
        format.source(),
        format.batch_type(),
        batch_name,
    )
}

/// Commit only the selected matched rows of a single-card import. Re-resolves
/// server-side (never trusts the preview) then keeps only the rows whose
/// `source_line` the user left selected. (dumpstar-oq3i.4)
///
/// When `park_unmatched` is set, the rows that didn't resolve at all are parked
/// in the dead-letter queue under this commit's batch, to resolve later
/// (dumpstar-oq3i.5).
pub fn commit_selected(
    conn: &mut Connection,
    format: ImportFormat,
    content: &str,
    include: &[u32],
    batch_name: Option<&str>,
    park_unmatched: bool,
) -> Result<CommitResult> {
    let rows = parse(format, content)?;
    let full = resolve(conn, &rows)?;
    let report = filter_report(&full, include);
    let result = commit_matched(
        conn,
        &report,
        format.source(),
        format.batch_type(),
        batch_name,
    )?;
    if park_unmatched {
        crate::unresolved::park_report(
            conn,
            format.source(),
            Some(result.batch_id),
            &rows,
            &full,
            &[],
            &SealedResolutionReport {
                matched: Vec::new(),
                unmatched: Vec::new(),
            },
        )?;
    }
    Ok(result)
}

/// Add every matched single-card row under a fresh batch. Shared by the
/// per-format [`commit`] and the combined [`commit_collectr`].
fn commit_matched(
    conn: &mut Connection,
    report: &ResolutionReport,
    source: &str,
    batch_type: &str,
    batch_name: Option<&str>,
) -> Result<CommitResult> {
    let batch_id = batches::create(
        conn,
        &NewBatch {
            batch_type: batch_type.to_string(),
            name: batch_name.map(str::to_string),
            ..Default::default()
        },
    )?;

    let mut added = 0u32;
    for r in &report.matched {
        let id = collection::add(
            conn,
            &NewCopy {
                printing_id: r.printing_id.clone(),
                condition: Some(r.condition.clone()),
                language: Some(r.language.clone()),
                purchase_price: r.purchase_price,
                acquired_at: r.acquired_at.clone(),
                source: source.to_string(),
                batch_id: Some(batch_id),
                ..Default::default()
            },
        )?;
        if !r.tags.is_empty() {
            collection::update(
                conn,
                id,
                &CopyEdit {
                    tags: Some(tags_json(&r.tags)),
                    ..Default::default()
                },
            )?;
        }
        added += 1;
    }

    Ok(CommitResult {
        batch_id,
        added,
        skipped: report.unmatched.len() as u32,
    })
}

// --- Collectr: the combined single + sealed import path (garden wall) ---

/// A row skipped during a combined import (e.g. another game's Collectr row).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SkippedRow {
    #[ts(type = "number")]
    pub source_line: u32,
    pub category: String,
    pub name: String,
    pub reason: String,
}

impl From<collectr::SkippedRow> for SkippedRow {
    fn from(s: collectr::SkippedRow) -> Self {
        Self {
            source_line: s.source_line as u32,
            category: s.category,
            name: s.name,
            reason: s.reason,
        }
    }
}

/// The preview of a combined import: the single and sealed resolutions kept
/// strictly apart (the garden wall), plus rows dropped as out of scope.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CombinedReport {
    pub singles: ResolutionReport,
    pub sealed: SealedResolutionReport,
    pub skipped: Vec<SkippedRow>,
}

/// The outcome of committing a combined import.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CombinedCommitResult {
    pub singles: CommitResult,
    pub sealed: SealedCommitResult,
    #[ts(type = "number")]
    pub skipped: u32,
}

/// Preview a Collectr import: split the file, then resolve singles and
/// sealed independently. Writes nothing.
pub fn preview_collectr(conn: &Connection, content: &str) -> Result<CombinedReport> {
    let parsed = collectr::parse(content)?;
    let mut singles = resolve(conn, &parsed.singles)?;
    annotate_owned(conn, &mut singles)?;
    let mut sealed = sealed_import::resolve_sealed(conn, &parsed.sealed)?;
    annotate_owned_sealed(conn, &mut sealed)?;
    Ok(CombinedReport {
        singles,
        sealed,
        skipped: parsed.skipped.into_iter().map(Into::into).collect(),
    })
}

/// Commit a Collectr import: split the file, re-resolve both halves
/// server-side, then write singles to `collection` (under a batch) and
/// sealed to `sealed_collection`. The two halves never cross.
pub fn commit_collectr(
    conn: &mut Connection,
    content: &str,
    batch_name: Option<&str>,
) -> Result<CombinedCommitResult> {
    let parsed = collectr::parse(content)?;
    let singles_report = resolve(conn, &parsed.singles)?;
    let sealed_report = sealed_import::resolve_sealed(conn, &parsed.sealed)?;

    let singles = commit_matched(
        conn,
        &singles_report,
        "csv_collectr",
        "csv_collectr",
        batch_name,
    )?;
    let sealed = sealed_import::commit_sealed(conn, &sealed_report, "csv_collectr")?;

    Ok(CombinedCommitResult {
        singles,
        sealed,
        skipped: parsed.skipped.len() as u32,
    })
}

/// Commit only the selected rows of a Collectr import. Re-resolves both halves
/// server-side, keeps only the `source_line`s the user selected in each pane,
/// then writes singles and sealed to their own tables (garden wall intact).
/// (dumpstar-oq3i.4)
pub fn commit_collectr_selected(
    conn: &mut Connection,
    content: &str,
    include_singles: &[u32],
    include_sealed: &[u32],
    batch_name: Option<&str>,
    park_unmatched: bool,
) -> Result<CombinedCommitResult> {
    let parsed = collectr::parse(content)?;
    let singles_full = resolve(conn, &parsed.singles)?;
    let sealed_full = sealed_import::resolve_sealed(conn, &parsed.sealed)?;
    let singles_report = filter_report(&singles_full, include_singles);
    let sealed_report = filter_sealed(&sealed_full, include_sealed);

    let singles = commit_matched(
        conn,
        &singles_report,
        "csv_collectr",
        "csv_collectr",
        batch_name,
    )?;
    let sealed = sealed_import::commit_sealed(conn, &sealed_report, "csv_collectr")?;

    if park_unmatched {
        // Both halves' misses go to the queue under the singles batch (the
        // import's provenance handle).
        crate::unresolved::park_report(
            conn,
            "csv_collectr",
            Some(singles.batch_id),
            &parsed.singles,
            &singles_full,
            &parsed.sealed,
            &sealed_full,
        )?;
    }

    Ok(CombinedCommitResult {
        singles,
        sealed,
        skipped: parsed.skipped.len() as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use crate::{connect_user, open_shared};

    const CSV: &str = "\
Set code,Set name,Collector number,Foil,Quantity,Condition,Language,Purchase price
SOR,Spark of Rebellion,5,normal,2,near_mint,en,1.50
SOR,Spark of Rebellion,5,foil,1,near_mint,en,5.00
NOPE,Mystery,1,normal,1,near_mint,en,";

    /// SOR #5 Luke Skywalker - Faithful Friend (Normal only) and SOR #90
    /// Vanquish, the leading-zero regression fixture (Collectr writes '090').
    fn db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::card_full(
                &c,
                "SOR-5",
                "Luke Skywalker",
                Some("Faithful Friend"),
                "Leader",
                None,
                Some(2),
                Some(7),
                "[\"Vigilance\",\"Heroism\"]",
                "[]",
                "[\"Ground\"]",
                None,
            );
            testkit::printing(
                &c,
                "SOR-5-normal",
                "SOR-5",
                "SOR",
                "5",
                "Standard",
                "Normal",
            );
            testkit::simple(&c, "SOR", "90", "Vanquish");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    fn row(set: &str, name: Option<&str>, number: &str, finish: &str) -> ParsedRow {
        ParsedRow {
            source_line: 2,
            set_hint: set.into(),
            set_name: Some(set.into()),
            name: name.map(str::to_string),
            number: number.into(),
            finish: finish.into(),
            condition: "Near Mint".into(),
            language: "English".into(),
            purchase_price: None,
            acquired_at: None,
            tags: Vec::new(),
        }
    }

    #[test]
    fn resolves_by_set_code_and_reports_misses() {
        let (_d, conn) = db();
        let report = preview(&conn, ImportFormat::Manabox, CSV).unwrap();

        // Two 'normal' copies of #5 resolve via the set code 'SOR'.
        assert_eq!(report.matched.len(), 2);
        assert_eq!(report.matched[0].set_code, "SOR");
        assert_eq!(
            report.matched[0].card_name,
            "Luke Skywalker - Faithful Friend"
        );
        assert_eq!(report.matched[0].finish, "Normal");
        assert_eq!(report.matched[0].variant, "Standard");
        assert_eq!(report.matched[0].purchase_price, Some(1.50));

        // The 'foil' copy (no Foil printing) and the unknown set both miss.
        assert_eq!(report.unmatched.len(), 2);
        assert!(
            report
                .unmatched
                .iter()
                .any(|u| u.reason.contains("finish 'Foil'") && u.finish == "Foil")
        );
        assert!(
            report
                .unmatched
                .iter()
                .any(|u| u.reason.contains("unknown set 'NOPE'"))
        );
    }

    #[test]
    fn foil_row_resolves_to_the_foil_printing() {
        let (dir, _conn) = db();
        {
            let c = open_shared(&dir.path().join("shared.sqlite")).unwrap();
            testkit::printing(&c, "SOR-5-foil", "SOR-5", "SOR", "5", "Standard", "Foil");
        }
        let conn = connect_user(
            &dir.path().join("collection.sqlite"),
            &dir.path().join("shared.sqlite"),
        )
        .unwrap();
        let report = preview(&conn, ImportFormat::Manabox, CSV).unwrap();
        assert_eq!(report.matched.len(), 3);
        assert_eq!(report.matched[2].printing_id, "SOR-5-foil");
        assert_eq!(report.matched[2].finish, "Foil");
        assert_eq!(report.unmatched.len(), 1);
    }

    #[test]
    fn commit_adds_matched_rows_under_a_batch() {
        let (_d, mut conn) = db();
        let result = commit(&mut conn, ImportFormat::Manabox, CSV, Some("import.csv")).unwrap();
        assert_eq!(result.added, 2);
        assert_eq!(result.skipped, 2);

        let cards = collection::list_by_batch(&conn, result.batch_id).unwrap();
        assert_eq!(cards.len(), 2);
        assert!(cards.iter().all(|c| c.source == "csv_manabox"));
    }

    #[test]
    fn zero_padded_number_resolves_against_unpadded_catalog() {
        // Collectr writes '090'; the catalog stores '90'. An exact WHERE
        // number='090' misses — the resolver must normalize both sides.
        let (_d, conn) = db();
        let report = resolve(&conn, &[row("SOR", None, "090", "Normal")]).unwrap();
        assert_eq!(report.matched.len(), 1, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].card_name, "Vanquish");
        assert_eq!(report.matched[0].printing_id, "SOR-90-normal");
        assert_eq!(report.matched[0].number, "90");
    }

    #[test]
    fn set_name_plus_leading_zero_resolves() {
        // Collectr sends the set's name ('Spark of Rebellion', not its code)
        // and a zero-padded number '005'; both normalizations must fire.
        let (_d, conn) = db();
        assert_eq!(
            resolve_set(&conn, "Spark of Rebellion", Some("Spark of Rebellion")).unwrap(),
            Some("SOR".to_string())
        );
        assert_eq!(
            resolve_set(&conn, "sor", None).unwrap(),
            Some("SOR".to_string())
        );
        assert_eq!(resolve_set(&conn, "Nowhere", None).unwrap(), None);

        let report = resolve(&conn, &[row("Spark of Rebellion", None, "005", "Normal")]).unwrap();
        assert_eq!(report.matched.len(), 1, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].set_code, "SOR");
        assert_eq!(
            report.matched[0].card_name,
            "Luke Skywalker - Faithful Friend"
        );
    }

    /// A catalog for the name+number fallback: Luke at SOR #5, Vanquish at
    /// SHD #26 (Normal and Foil), a different card at SOR #26.
    fn misc_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::set(&c, "SHD", "Shadows of the Galaxy");
            testkit::card_full(
                &c,
                "SOR-5",
                "Luke Skywalker",
                Some("Faithful Friend"),
                "Leader",
                None,
                None,
                None,
                "[]",
                "[]",
                "[]",
                None,
            );
            testkit::printing(
                &c,
                "SOR-5-normal",
                "SOR-5",
                "SOR",
                "5",
                "Standard",
                "Normal",
            );
            testkit::simple(&c, "SOR", "26", "Cartel Spoils");
            testkit::simple(&c, "SHD", "26", "Vanquish");
            testkit::foil_of(&c, "SHD", "26");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    fn misc_row(name: &str, number: &str, finish: &str) -> ParsedRow {
        row("Miscellaneous Products", Some(name), number, finish)
    }

    #[test]
    fn name_with_subtitle_resolves_across_sets() {
        // The set doesn't resolve; "Name - Subtitle" plus the number falls
        // back to SOR #5.
        let (_d, conn) = misc_db();
        let report = resolve(
            &conn,
            &[misc_row(
                "Luke Skywalker - Faithful Friend",
                "005",
                "Normal",
            )],
        )
        .unwrap();
        assert_eq!(report.matched.len(), 1, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].set_code, "SOR");
        assert_eq!(report.matched[0].printing_id, "SOR-5-normal");
    }

    #[test]
    fn bare_name_matches_a_card_with_a_subtitle() {
        let (_d, conn) = misc_db();
        let report = resolve(&conn, &[misc_row("Luke Skywalker", "5", "Normal")]).unwrap();
        assert_eq!(report.matched.len(), 1, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].printing_id, "SOR-5-normal");
    }

    #[test]
    fn stamped_promo_resolves_by_name_and_number_across_sets() {
        // "Vanquish (Prerelease Promo)" #026 Foil → SHD #26 Foil (leading-zero
        // strip + descriptor strip), never SOR #26 (Cartel Spoils).
        let (_d, conn) = misc_db();
        let report = resolve(
            &conn,
            &[misc_row("Vanquish (Prerelease Promo)", "026", "Foil")],
        )
        .unwrap();
        assert_eq!(report.matched.len(), 1, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].set_code, "SHD");
        assert_eq!(report.matched[0].number, "26");
        assert_eq!(report.matched[0].printing_id, "SHD-26-foil");
    }

    #[test]
    fn missing_finish_is_reported() {
        let (_d, conn) = misc_db();
        let report = resolve(&conn, &[misc_row("Cartel Spoils", "26", "Foil")]).unwrap();
        assert!(report.matched.is_empty());
        assert_eq!(report.unmatched.len(), 1);
        assert!(
            report.unmatched[0]
                .reason
                .contains("finish 'Foil' not available"),
            "{}",
            report.unmatched[0].reason
        );
    }

    #[test]
    fn ambiguous_name_number_is_flagged_not_guessed() {
        // Two sets carry a card with the same (name, number): the fallback
        // must report the candidates for manual resolution, not pick one.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "5", "Rebel Trooper");
            testkit::simple(&c, "SHD", "5", "Rebel Trooper");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let report = resolve(&conn, &[misc_row("Rebel Trooper", "5", "Normal")]).unwrap();
        assert!(report.matched.is_empty());
        assert_eq!(report.unmatched.len(), 1);
        let reason = &report.unmatched[0].reason;
        assert!(reason.contains("ambiguous"), "{reason}");
        assert!(
            reason.contains("SOR #5") && reason.contains("SHD #5"),
            "{reason}"
        );
    }

    #[test]
    fn hyperspace_number_resolves_to_the_hyperspace_printing() {
        // Standard #5 and Hyperspace #262 are different collector numbers of
        // the same card: the number carries the variant.
        let (dir, _conn) = db();
        {
            let c = open_shared(&dir.path().join("shared.sqlite")).unwrap();
            testkit::printing(
                &c,
                "SOR-5-hs",
                "SOR-5",
                "SOR",
                "262",
                "Hyperspace",
                "Normal",
            );
        }
        let conn = connect_user(
            &dir.path().join("collection.sqlite"),
            &dir.path().join("shared.sqlite"),
        )
        .unwrap();
        let report = resolve(
            &conn,
            &[
                row("SOR", None, "5", "Normal"),
                row("SOR", None, "262", "Normal"),
            ],
        )
        .unwrap();
        assert_eq!(report.matched.len(), 2, "{:?}", report.unmatched);
        assert_eq!(report.matched[0].variant, "Standard");
        assert_eq!(report.matched[1].variant, "Hyperspace");
        assert_eq!(report.matched[1].printing_id, "SOR-5-hs");
    }

    #[test]
    fn preview_flags_already_owned_copies() {
        let (_d, mut conn) = db();
        // Nothing owned yet — every matched row reads 0.
        let report = preview(&conn, ImportFormat::Manabox, CSV).unwrap();
        assert!(report.matched.iter().all(|r| r.already_owned == 0));

        // Own one copy of the normal printing, then re-preview.
        collection::add(
            &mut conn,
            &NewCopy {
                printing_id: "SOR-5-normal".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let report = preview(&conn, ImportFormat::Manabox, CSV).unwrap();
        assert!(!report.matched.is_empty());
        assert!(
            report.matched.iter().all(|r| r.already_owned == 1),
            "matched normal rows should report already_owned == 1"
        );
    }

    #[test]
    fn commit_selected_adds_only_included_lines() {
        let (_d, mut conn) = db();
        // Selecting nothing adds nothing.
        let none =
            commit_selected(&mut conn, ImportFormat::Manabox, CSV, &[], None, false).unwrap();
        assert_eq!(none.added, 0);

        // Line 2 is the pair of matched 'normal' copies; including it adds both.
        let some = commit_selected(
            &mut conn,
            ImportFormat::Manabox,
            CSV,
            &[2],
            Some("sel.csv"),
            false,
        )
        .unwrap();
        assert_eq!(some.added, 2);
        let cards = collection::list_by_batch(&conn, some.batch_id).unwrap();
        assert_eq!(cards.len(), 2);
    }

    #[test]
    fn unknown_format_is_rejected() {
        assert!(ImportFormat::parse("nonsense").is_err());
        // Collectr is now a recognized format.
        assert_eq!(
            ImportFormat::parse("collectr").unwrap(),
            ImportFormat::Collectr
        );
    }

    const COLLECTR_CSV: &str = "\
Portfolio Name,Category,Set,Product Name,Card Number,Rarity,Variance,Grade,Card Condition,Average Cost Paid,Quantity,Market Price (As of 2026-07-17),Price Override,Watchlist,Date Added,Notes
Main,Lorcana,Disney Lorcana Promo Cards,Elsa,6,Promo,Holofoil,Ungraded,Near Mint,0,1,7.12,0,false,2026-01-24,
Main,Star Wars Unlimited,Spark of Rebellion,Luke Skywalker - Faithful Friend,005/252,Legendary,Normal,Ungraded,Near Mint,1.50,2,5.00,0,false,2026-04-14,
Sealed Star Wars Unlimited,Star Wars Unlimited,Spark of Rebellion,Spark of Rebellion Booster Box,,,Normal,Ungraded,Near Mint,49.99,3,60.00,0,false,2026-02-28,";

    fn sealed_db() -> (tempfile::TempDir, Connection) {
        let (dir, conn) = db();
        drop(conn);
        {
            let c = open_shared(&dir.path().join("shared.sqlite")).unwrap();
            c.execute(
                "INSERT INTO sealed_products (product_id, set_code, name, category, fetched_at) \
                 VALUES (7001, 'SOR', 'Spark of Rebellion Booster Box', 'sealed', '2026-02-28')",
                [],
            )
            .unwrap();
        }
        let conn = connect_user(
            &dir.path().join("collection.sqlite"),
            &dir.path().join("shared.sqlite"),
        )
        .unwrap();
        (dir, conn)
    }

    #[test]
    fn collectr_preview_splits_singles_sealed_and_skips() {
        let (_d, conn) = sealed_db();
        let report = preview_collectr(&conn, COLLECTR_CSV).unwrap();

        // Two 'Normal' Luke copies resolve; set 'Spark of Rebellion' matches by name.
        assert_eq!(report.singles.matched.len(), 2);
        assert_eq!(
            report.singles.matched[0].card_name,
            "Luke Skywalker - Faithful Friend"
        );
        assert_eq!(
            report.singles.matched[0].acquired_at.as_deref(),
            Some("2026-04-14T00:00:00Z")
        );
        // The booster box resolves on the sealed side, quantity 3 preserved.
        assert_eq!(report.sealed.matched.len(), 1);
        assert_eq!(report.sealed.matched[0].quantity, 3);
        // Lorcana row is skipped.
        assert_eq!(report.skipped.len(), 1);
        assert_eq!(report.skipped[0].category, "Lorcana");
    }

    #[test]
    fn collectr_commit_writes_both_sides_without_crossing() {
        let (_d, mut conn) = sealed_db();
        let result = commit_collectr(&mut conn, COLLECTR_CSV, Some("collectr.csv")).unwrap();
        assert_eq!(result.singles.added, 2);
        assert_eq!(result.sealed.added, 1);
        assert_eq!(result.skipped, 1);

        // Singles landed in `collection`, sealed in `sealed_collection` —
        // one box row with quantity 3, never leaked into singles.
        let singles = collection::list_by_batch(&conn, result.singles.batch_id).unwrap();
        assert_eq!(singles.len(), 2);
        assert!(singles.iter().all(|c| c.source == "csv_collectr"));

        let sealed = crate::sealed::list(&conn).unwrap();
        assert_eq!(sealed.len(), 1);
        assert_eq!(sealed[0].quantity, 3);
    }

    #[test]
    fn collectr_preview_annotates_owned_on_both_sides() {
        let (_d, mut conn) = sealed_db();
        // Own one Luke copy and the box up front.
        collection::add(
            &mut conn,
            &NewCopy {
                printing_id: "SOR-5-normal".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();
        crate::sealed::add(
            &conn,
            &crate::sealed::NewSealed {
                product_id: 7001,
                quantity: Some(2),
                ..Default::default()
            },
        )
        .unwrap();

        let report = preview_collectr(&conn, COLLECTR_CSV).unwrap();
        assert!(report.singles.matched.iter().all(|r| r.already_owned == 1));
        assert_eq!(report.sealed.matched[0].already_owned, 2);
    }

    #[test]
    fn collectr_commit_selected_honors_each_pane() {
        let (_d, mut conn) = sealed_db();
        // Take the sealed line (4) only; leave the singles line (3) out.
        let r = commit_collectr_selected(&mut conn, COLLECTR_CSV, &[], &[4], None, false).unwrap();
        assert_eq!(r.singles.added, 0);
        assert_eq!(r.sealed.added, 1);

        // Now take only the singles line.
        let r = commit_collectr_selected(&mut conn, COLLECTR_CSV, &[3], &[], None, false).unwrap();
        assert_eq!(r.singles.added, 2);
        assert_eq!(r.sealed.added, 0);
    }
}
