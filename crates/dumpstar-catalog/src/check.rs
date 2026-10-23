//! The alarms the catalog can raise, read from a (read-only) catalog: what the
//! join recorded, a price day that never arrived, and a day too thinly priced.
//! `dumpstar catalog-check` runs [`run`] from a timer and fails when it returns
//! anything, which is what pages through the unit's `OnFailure=`.

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use rusqlite::Connection;

use crate::crosswalk::PRICED_VARIANTS;
use crate::error::Result;
use crate::record::AlarmRecord;

/// tcgcsv refreshes around 20:00 UTC and the sync timer runs at 21:00; a day is
/// due once the clock passes this hour, an hour of slack after the sync starts.
pub const PRICES_DUE_HOUR_UTC: u32 = 22;
/// How far back a missing day is still reported.
pub const LOOKBACK_DAYS: i64 = 30;
/// The share of priced-variant printings that must have a price on the latest day.
pub const MIN_COVERAGE: f64 = 0.99;

/// Alarms the join stored (severity 'alarm' only).
pub fn stored(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut stmt = conn.prepare(
        "SELECT kind, key, severity, detail FROM alarms WHERE severity = 'alarm' ORDER BY kind, key",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(AlarmRecord {
                kind: r.get(0)?,
                key: r.get(1)?,
                severity: r.get(2)?,
                detail: r.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Every day from the first priced day (or `LOOKBACK_DAYS` ago) up to the last
/// day that is due at `now` and has no price row.
pub fn missing_price_days(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<AlarmRecord>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT observed_on FROM prices WHERE source = 'tcgcsv'")?;
    let days: std::collections::BTreeSet<NaiveDate> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|d| NaiveDate::parse_from_str(&d.ok()?, "%Y-%m-%d").ok())
        .collect();
    let Some(&first) = days.iter().next() else {
        return Ok(vec![]);
    };
    let today = now.date_naive();
    let due = if now.hour() >= PRICES_DUE_HOUR_UTC {
        today
    } else {
        today - Duration::days(1)
    };
    let mut d = first.max(today - Duration::days(LOOKBACK_DAYS));
    let mut out = vec![];
    while d <= due {
        if !days.contains(&d) {
            out.push(AlarmRecord::alarm(
                "missing_price_day",
                d.to_string(),
                format!(
                    "no tcgcsv price row for {d}; the latest is {}",
                    days.iter().next_back().unwrap()
                ),
            ));
        }
        d += Duration::days(1);
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Coverage {
    pub eligible: i64,
    pub priced: i64,
}

impl Coverage {
    pub fn ratio(&self) -> f64 {
        if self.eligible == 0 {
            1.0
        } else {
            self.priced as f64 / self.eligible as f64
        }
    }
}

/// Printings of priced variants in sets tcgcsv covers (tokens excluded), and how
/// many of them have a price row on `day`.
pub fn coverage(conn: &Connection, day: &str) -> Result<Coverage> {
    let variants = PRICED_VARIANTS
        .iter()
        .map(|v| format!("'{v}'"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT COUNT(*), COALESCE(SUM(EXISTS (SELECT 1 FROM prices x
            WHERE x.printing_id = p.printing_id AND x.source = 'tcgcsv' AND x.observed_on = ?1)), 0)
         FROM printings p
         WHERE p.variant IN ({variants}) AND p.number NOT GLOB '[Tt][0-9]*'
           AND p.set_code IN (SELECT DISTINCT p2.set_code FROM printings p2
                JOIN printing_source_id s ON s.printing_id = p2.printing_id AND s.source = 'tcgcsv')"
    );
    Ok(conn.query_row(&sql, [day], |r| {
        Ok(Coverage {
            eligible: r.get(0)?,
            priced: r.get(1)?,
        })
    })?)
}

/// How many of a set's printings carry a tcgcsv product id (tokens aside: a
/// token is priced as the back of a base or not at all).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetCoverage {
    pub set_code: String,
    pub printings: i64,
    pub linked: i64,
}

impl SetCoverage {
    pub fn percent(&self) -> i64 {
        if self.printings == 0 {
            100
        } else {
            self.linked * 100 / self.printings
        }
    }
}

pub fn set_coverage(conn: &Connection) -> Result<Vec<SetCoverage>> {
    let mut stmt = conn.prepare(
        "SELECT p.set_code, COUNT(*),
                COALESCE(SUM(EXISTS (SELECT 1 FROM printing_source_id s
                    WHERE s.printing_id = p.printing_id AND s.source = 'tcgcsv')), 0)
         FROM printings p
         WHERE p.number NOT GLOB '[Tt][0-9]*'
         GROUP BY p.set_code ORDER BY p.set_code",
    )?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SetCoverage {
                set_code: r.get(0)?,
                printings: r.get(1)?,
                linked: r.get(2)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// Each set's floor from `coverage_floor.tsv`.
pub fn coverage_floors() -> Vec<(String, i64)> {
    include_str!("coverage_floor.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some((f.next()?.trim().to_string(), f.next()?.trim().parse().ok()?))
        })
        .collect()
}

/// A set whose coverage has fallen below its floor.
pub fn coverage_drops(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let floors = coverage_floors();
    Ok(set_coverage(conn)?
        .into_iter()
        .filter_map(|c| {
            let floor = floors.iter().find(|(s, _)| *s == c.set_code)?.1;
            (c.percent() < floor).then(|| {
                AlarmRecord::alarm(
                    "set_coverage_dropped",
                    c.set_code.clone(),
                    format!(
                        "{}: {} of {} printings carry a tcgcsv product id ({}%, floor {floor}%)",
                        c.set_code,
                        c.linked,
                        c.printings,
                        c.percent()
                    ),
                )
            })
        })
        .collect())
}

/// Every printing with no tcgcsv product id, and why the join left it: a token,
/// or the alarm it raised.
pub fn unmatched(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT p.set_code, p.variant, p.finish, p.number, c.name, COALESCE(c.subtitle, ''),
                p.printing_id,
                COALESCE((SELECT a.kind FROM alarms a WHERE a.key = p.printing_id LIMIT 1), '')
         FROM printings p JOIN cards c ON c.card_id = p.card_id
         WHERE NOT EXISTS (SELECT 1 FROM printing_source_id s
                WHERE s.printing_id = p.printing_id AND s.source = 'tcgcsv')
         ORDER BY p.set_code, p.number, p.variant, p.finish",
    )?;
    let rows = stmt
        .query_map([], |r| {
            let (set, variant, finish, number, name, sub, id, kind): (
                String,
                String,
                String,
                String,
                String,
                String,
                String,
                String,
            ) = (
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            );
            let title = if sub.is_empty() {
                name
            } else {
                format!("{name} - {sub}")
            };
            let why = match kind.as_str() {
                "" if number.starts_with(['t', 'T']) => "token",
                "" => "no tcgcsv product",
                k => k,
            };
            Ok(format!(
                "{set} {variant} {finish} #{number} {title} [{id}]: {why}"
            ))
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// The share of sealed pictures still unstored above which `catalog-check`
/// alarms; the apply job only reports them (sp-lgdp0x).
pub const MAX_MISSING_SEALED_IMAGES: f64 = 0.25;

pub fn missing_sealed_images(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let (total, missing): (i64, i64) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(retrieved_on IS NULL), 0) FROM sealed_images",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if total > 0 && missing as f64 / total as f64 > MAX_MISSING_SEALED_IMAGES {
        return Ok(vec![AlarmRecord::alarm(
            "sealed_images_missing",
            "sealed_images".to_string(),
            format!("{missing} of {total} sealed product pictures not stored"),
        )]);
    }
    Ok(vec![])
}

/// Below this many cards with rules text a catalog is a fixture, not a sync.
const MIN_CARDS_FOR_TEXT_CHECK: i64 = 50;

/// A catalog with text but no structured text: `k:` and `has:epic` match
/// nothing and the agent tools return empty keywords (sp-nb4vqy). The columns
/// were added in place to a catalog whose payloads were already applied, and
/// nothing re-derived them.
pub fn missing_structured_text(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let (texted, keyworded, epic): (i64, i64, i64) = conn.query_row(
        "SELECT COALESCE(SUM(rules_text IS NOT NULL AND rules_text != ''), 0),
                COALESCE(SUM(keywords != '[]'), 0),
                COALESCE(SUM(epic_action IS NOT NULL AND epic_action != ''), 0)
         FROM cards",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    if texted >= MIN_CARDS_FOR_TEXT_CHECK && (keyworded == 0 || epic == 0) {
        return Ok(vec![AlarmRecord::alarm(
            "structured_text_missing",
            "cards".to_string(),
            format!(
                "{texted} cards have rules text but {keyworded} carry keywords and {epic} an epic action"
            ),
        )]);
    }
    Ok(vec![])
}

/// Rules text that cannot be the printed card: a clause stated twice, or none
/// at all on a card type that always prints some. A unit may be vanilla or a
/// lone keyword, so units are not held to it; tokens never are.
pub fn suspect_rules_text(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT card_id, card_type, rules_text FROM cards
         WHERE card_type NOT LIKE '%Token%' ORDER BY card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, ty, rules) = row?;
        let rules = rules.unwrap_or_default();
        if let Some(dup) = repeated_sentence(&rules) {
            out.push(AlarmRecord::alarm(
                "rules_text_repeated",
                id.clone(),
                format!("{dup:?} appears twice in the rules text"),
            ));
        }
        if crate::official::has_markup(&rules) {
            out.push(AlarmRecord::alarm(
                "rules_text_markup",
                id.clone(),
                format!("{rules:?} still holds a <tag>"),
            ));
        }
        if let Some(why) = crate::official::icon_defect(&rules).filter(|w| *w != "raw icon markup")
        {
            out.push(AlarmRecord::alarm(
                "rules_text_icon",
                id.clone(),
                format!("{rules:?}: {why}"),
            ));
        }
        let always_texted = matches!(ty.as_deref(), Some("Event" | "Upgrade" | "Leader" | "Base"));
        if always_texted && rules.split_whitespace().count() <= 1 {
            out.push(AlarmRecord::info(
                "rules_text_sparse",
                id,
                format!(
                    "{} card has {rules:?} for rules text",
                    ty.unwrap_or_default()
                ),
            ));
        }
    }
    Ok(out)
}

/// A keyword stored with no number although the card's text gives it one:
/// `Restore` null on a card that says "gains Restore 2" (sp-8p9a57).
pub fn keyword_number_missing(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT card_id, keywords, rules_text FROM cards
         WHERE keywords != '[]' AND rules_text IS NOT NULL ORDER BY card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, keywords, rules) = row?;
        let Ok(serde_json::Value::Array(kws)) = serde_json::from_str(&keywords) else {
            continue;
        };
        for k in kws {
            let Some(name) = k.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            if k.get("value").is_some_and(|v| !v.is_null()) {
                continue;
            }
            let numbered = rules.match_indices(name).any(|(at, _)| {
                let before = rules[..at].chars().next_back();
                let rest = rules[at + name.len()..].strip_prefix(' ');
                before.is_none_or(|c| !c.is_alphanumeric())
                    && rest.is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit()))
            });
            if numbered {
                out.push(AlarmRecord::alarm(
                    "keyword_number_missing",
                    id.clone(),
                    format!("{name} has a number in the rules text but a null value"),
                ));
            }
        }
    }
    Ok(out)
}

/// A known keyword that stands at the start of a line or list item of the rules
/// text but is absent from the card's keywords (Support and Plot were, sp-pljdpa).
/// A keyword found only mid-sentence is one the card grants (sp-wxj6lr).
pub fn keyword_missing_from_keywords(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT card_id, keywords, rules_text FROM cards
         WHERE rules_text IS NOT NULL ORDER BY card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, keywords, rules) = row?;
        let have: Vec<String> = match serde_json::from_str(&keywords) {
            Ok(serde_json::Value::Array(kws)) => kws
                .iter()
                .filter_map(|k| k.get("name")?.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        };
        for name in crate::official::KNOWN_KEYWORDS {
            if crate::official::stands_alone(&rules, name) && !have.iter().any(|h| h == name) {
                out.push(AlarmRecord::alarm(
                    "keyword_missing",
                    id.clone(),
                    format!("{name} stands in the rules text but is not among the keywords"),
                ));
            }
        }
    }
    Ok(out)
}

/// A card whose rules text mentions a mechanic-like term (the Force, the
/// initiative, a token) that its stored mechanics do not account for: new
/// wording outside the derivation's vocabulary (sp-1qcjva). Not an outage: the
/// finding is `info`, and `catalog-check` files it as a vocabulary-fix bead
/// ([`crate::filing`]) instead of failing the unit.
pub fn mechanic_unmatched(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    Ok(mechanic_findings(conn)?
        .into_iter()
        .map(|f| {
            AlarmRecord::info(
                "mechanic_unmatched",
                f.card_id,
                format!(
                    "the rules text says \"{}\" but derives no mechanic for it",
                    f.phrase
                ),
            )
        })
        .collect())
}

/// Every (card, unmatched phrase) with the card's rules text.
pub fn mechanic_findings(conn: &Connection) -> Result<Vec<crate::filing::Finding>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT card_id, mechanics, rules_text FROM cards
         WHERE rules_text IS NOT NULL ORDER BY card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, mechanics, rules) = row?;
        let have: Vec<crate::mechanics::MechanicRecord> =
            serde_json::from_str(&mechanics).unwrap_or_default();
        for phrase in crate::mechanics::unmatched_terms(Some(&rules), &have) {
            out.push(crate::filing::Finding {
                card_id: id.clone(),
                rules_text: rules.clone(),
                phrase,
            });
        }
    }
    Ok(out)
}

/// A stored keyword whose granted or conditional flag disagrees with the one
/// rule its rules text derives (sp-gww4nf): own, conditional own, granted to
/// another unit, or removed ("lose X") and so not stored at all.
pub fn keyword_misclassified(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    use crate::official::{Standing, standing};
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT card_id, keywords, rules_text FROM cards
         WHERE keywords != '[]' AND rules_text IS NOT NULL ORDER BY card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    for row in rows {
        let (id, keywords, rules) = row?;
        let Ok(serde_json::Value::Array(kws)) = serde_json::from_str(&keywords) else {
            continue;
        };
        for k in kws {
            let Some(name) = k.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let flag = |f: &str| k.get(f).and_then(|v| v.as_bool()).unwrap_or(false);
            let (granted, conditional) = (flag("granted"), flag("conditional"));
            let want = match standing(&rules, name) {
                Standing::Own { conditional } => Some((false, conditional)),
                Standing::Granted => Some((true, false)),
                Standing::Absent => None,
            };
            if want != Some((granted, conditional)) {
                out.push(AlarmRecord::alarm(
                    "keyword_misclassified",
                    id.clone(),
                    format!(
                        "{name} is stored granted={granted} conditional={conditional} but its rules text gives {want:?}"
                    ),
                ));
            }
        }
    }
    Ok(out)
}

/// A line of at least ten characters that occurs twice. A clause repeated
/// inside a line is a card that says it twice ("It costs 1 resource less" in
/// each branch of a choice); the importer's defect was a whole line stated twice.
fn repeated_sentence(rules: &str) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    for part in rules.split('\n').map(str::trim) {
        if part.len() < 10 {
            continue;
        }
        if seen.contains(&part) {
            return Some(part.to_string());
        }
        seen.push(part);
    }
    None
}

/// The numbering invariants of the printings table, as alarms.
///
/// Within a set code a collector number names one card per variant
/// (`number_collision`): a promo run filed under its parent set, as `SOR` once
/// held a Weekly Play Battlefield Marine at the number of Director Krennic,
/// breaks it. And a main expansion's Standard printings are numbered exactly
/// 1..N (`standard_numbering_gap`), tokens (`T01`) aside. Only the Normal
/// finish is held to it: the official API numbers a Standard Foil of a main
/// expansion in a run of its own, past the set's printed card count (JTL 525
/// is the foil of a card numbered below 262), and not the Rose Gold and Gold
/// Serialized parallels (variant codes 36, 37) the official API files as
/// Standard at SEC's Serialized Prestige numbers.
pub fn numbering(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT set_code, number, variant, COUNT(DISTINCT card_id) FROM printings
         GROUP BY set_code, number, variant HAVING COUNT(DISTINCT card_id) > 1
         ORDER BY set_code, number, variant",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, i64>(3)?,
        ))
    })? {
        let (set, number, variant, cards) = row?;
        out.push(AlarmRecord::alarm(
            "number_collision",
            format!("{set}/{number}/{variant}"),
            format!("{cards} different cards share {variant} number {number} in {set}"),
        ));
    }
    let mut stmt = conn.prepare(
        "SELECT p.set_code, CAST(p.number AS INTEGER) FROM printings p
           JOIN sets s ON s.set_code = p.set_code
          WHERE s.category = 'Main Expansions' AND p.variant = 'Standard'
            AND p.finish = 'Normal' AND COALESCE(p.variant_code, '') NOT IN ('36', '37')
            AND p.number NOT GLOB '*[^0-9]*' AND p.number <> ''
          GROUP BY p.set_code, CAST(p.number AS INTEGER)
          ORDER BY p.set_code, 2",
    )?;
    let mut by_set: std::collections::BTreeMap<String, Vec<i64>> = Default::default();
    for row in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (set, n) = row?;
        by_set.entry(set).or_default().push(n);
    }
    for (set, nums) in by_set {
        let max = *nums.last().unwrap();
        if nums.first() != Some(&1) || nums.len() as i64 != max {
            let missing: Vec<String> = (1..=max)
                .filter(|n| nums.binary_search(n).is_err())
                .take(10)
                .map(|n| n.to_string())
                .collect();
            out.push(AlarmRecord::alarm(
                "standard_numbering_gap",
                set.clone(),
                format!(
                    "{set}: {} Standard numbers up to {max}, expected 1..{max}; missing {}",
                    nums.len(),
                    missing.join(", ")
                ),
            ));
        }
    }
    Ok(out)
}

/// Data-lint over what the sources recorded, not what the join did.
///
/// `number_out_of_range`: a main expansion's Standard printing numbered above
/// twice the set's median Standard Normal number, which no printed card count
/// of the set reaches (the rest of the set is numbered 1..N; see [`numbering`]).
/// `truncated_subtitle`: a card whose subtitle ends in an ellipsis, which the
/// source cut short (the official record of Jar Jar Binks carries
/// "Mesa Propose…" on every one of its printings).
pub fn datalint(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut out = Vec::new();
    let mut stmt = conn.prepare(
        "SELECT p.set_code, p.number, CAST(p.number AS INTEGER) FROM printings p
           JOIN sets s ON s.set_code = p.set_code
          WHERE s.category = 'Main Expansions' AND p.variant = 'Standard'
            AND p.finish = 'Normal' AND COALESCE(p.variant_code, '') NOT IN ('36', '37')
            AND p.number NOT GLOB '*[^0-9]*' AND p.number <> ''
          ORDER BY p.set_code, 3",
    )?;
    let mut by_set: std::collections::BTreeMap<String, Vec<(String, i64)>> = Default::default();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })? {
        let (set, text, n) = row?;
        by_set.entry(set).or_default().push((text, n));
    }
    for (set, mut nums) in by_set {
        nums.dedup_by_key(|(_, n)| *n);
        let median = nums[nums.len() / 2].1;
        for (text, n) in nums.iter().filter(|(_, n)| *n > 2 * median) {
            out.push(AlarmRecord::alarm(
                "number_out_of_range",
                format!("{set}/{text}"),
                format!("{set} Standard number {n} is more than twice the set's median {median}"),
            ));
        }
    }
    let mut stmt = conn.prepare(
        "SELECT card_id, name, subtitle FROM cards WHERE subtitle LIKE '%…' ORDER BY card_id",
    )?;
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (id, name, subtitle) = row?;
        out.push(AlarmRecord::alarm(
            "truncated_subtitle",
            id,
            format!("{name}: subtitle {subtitle:?} ends in an ellipsis"),
        ));
    }
    Ok(out)
}

/// A printing linked to a TCGplayer product of a group that is not its set's
/// (`wrong_group`): the set's code was taken by a printing the group files
/// elsewhere. A promo group (`*`) carries any set; a Weekly Play group `XYZP`
/// also carries the Hyperspace printings of `XYZ`.
pub fn wrong_group(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let rows: Vec<(i64, String, Option<String>)> = conn
        .prepare("SELECT group_id, name, abbreviation FROM tcgcsv_groups")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let set_codes: std::collections::BTreeSet<String> = conn
        .prepare("SELECT set_code FROM sets")?
        .query_map([], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let groups = crate::tcgcsv::group_infos(&rows, &set_codes);
    let mut stmt = conn.prepare(
        "SELECT DISTINCT p.printing_id, p.set_code, p.number, p.variant, t.group_id
           FROM printing_source_id s
           JOIN printings p ON p.printing_id = s.printing_id
           JOIN tcgcsv_products t ON t.product_id = CAST(s.source_id AS INTEGER)
          WHERE s.source = 'tcgcsv' ORDER BY p.printing_id",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, i64>(4)?,
        ))
    })? {
        let (id, set, number, variant, group) = row?;
        let Some(info) = groups.get(&group) else {
            continue;
        };
        let home = info.set_code.as_str();
        if home == "*"
            || home == set
            || (variant == "Hyperspace" && home.strip_suffix('P') == Some(set.as_str()))
        {
            continue;
        }
        out.push(AlarmRecord::alarm(
            "wrong_group",
            id,
            format!(
                "{set} {variant} {number} is priced from TCGplayer group {group}, which belongs to {home}"
            ),
        ));
    }
    Ok(out)
}

/// The file name of a scan, without extension: `card_0131048en_base_Consular_Security_Force_Foil`.
fn image_stem(url: &str) -> Option<&str> {
    let file = url.rsplit('/').next()?.split(['?', '#']).next()?;
    Some(file.rsplit_once('.').map_or(file, |(s, _)| s))
}

/// A printing's image names another card (`image_wrong_card`): SOR #48 Foil's
/// upstream scan is `..._Consular_Security_Force_Foil`. The upstream names carry
/// typos of their own (`Jeddha`, `Liutenant`), so a name that merely differs
/// does not alarm; one that names a different card of the catalog does.
pub fn image_wrong_card(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    use crate::crosswalk::name_key;
    let others: Vec<(String, String)> = conn
        .prepare("SELECT DISTINCT name FROM cards")?
        .query_map([], |r| r.get::<_, String>(0))?
        .filter_map(|n| n.ok())
        .map(|n| (name_key(&n), n))
        .filter(|(k, _)| k.len() >= 8)
        .collect();
    let mut stmt = conn.prepare(
        "SELECT p.printing_id, c.name, p.image_url, p.set_code, p.number, p.finish
           FROM printings p JOIN cards c ON c.card_id = p.card_id
          WHERE p.image_url IS NOT NULL ORDER BY p.printing_id",
    )?;
    let mut out = Vec::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    })? {
        let (id, name, url, set, number, finish) = row?;
        let Some(stem) = image_stem(&url) else {
            continue;
        };
        let key = name_key(stem);
        if key.contains(&name_key(&name)) {
            continue;
        }
        if let Some((_, other)) = others.iter().find(|(k, _)| key.contains(k.as_str())) {
            out.push(AlarmRecord::alarm(
                "image_wrong_card",
                id,
                format!("{set} {number} {finish} is {name}, but its image {stem} is {other}"),
            ));
        }
    }
    Ok(out)
}

/// A printing, of any set, whose scan is the scan of another printing of its
/// card at a different set or number (`variant_image_shared`): a Hyperspace,
/// Showcase or prestige printing (`ASH` 535+, `LAWP` 21-40) has art of its own,
/// and a derivation that fell back to the Standard card's scan shows the wrong
/// card face. A foil sharing its Normal twin's scan (same set and number) is
/// the design, not a finding. The record's key is the printing, its message the
/// set, so a report groups per set.
pub fn variant_image_shared(conn: &Connection) -> Result<Vec<AlarmRecord>> {
    let mut stmt = conn.prepare(
        "SELECT DISTINCT p.printing_id, p.set_code, p.number, p.variant, p.finish,
                o.set_code, o.number, o.variant
           FROM printings p
           JOIN printings o ON o.card_id = p.card_id AND o.image_url = p.image_url
                           AND o.printing_id <> p.printing_id
                           AND (o.set_code <> p.set_code OR o.number <> p.number)
          WHERE p.image_url IS NOT NULL AND p.variant <> 'Standard'
          ORDER BY p.printing_id, o.set_code, o.number",
    )?;
    let mut out: Vec<AlarmRecord> = Vec::new();
    for row in stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    })? {
        let (id, set, number, variant, finish, oset, onumber, ovariant) = row?;
        if out.last().is_some_and(|a| a.key == id) {
            continue;
        }
        out.push(AlarmRecord::alarm(
            "variant_image_shared",
            id,
            format!(
                "{set} {variant} {number} {finish} shows the same image as {oset} {ovariant} {onumber}"
            ),
        ));
    }
    Ok(out)
}

/// What one check pass found: the alarms that fail the unit, the known-accepted
/// ones (with the reason each is accepted), and the notes that never page.
#[derive(Debug, Default)]
pub struct Report {
    pub open: Vec<AlarmRecord>,
    pub accepted: Vec<(AlarmRecord, String)>,
    pub info: Vec<AlarmRecord>,
}

/// Rows of `accepted_alarms.tsv`: `kind`, `key`, `reason`. A row accepts the one
/// alarm of that kind and key and nothing else; a new key alarms until someone
/// has read it and added a row (or fixed the data).
pub fn accepted_table() -> Vec<(String, String, String)> {
    include_str!("accepted_alarms.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some((
                f.next()?.trim().to_string(),
                f.next()?.trim().to_string(),
                f.next()?.trim().to_string(),
            ))
        })
        .collect()
}

/// An official printing no tcgcsv product matches, in a set that is not on sale
/// yet: tcgcsv lists a set once it is, so the gap is expected until its release
/// date (`official_without_tcgcsv` only; every other kind still alarms).
fn unreleased_set_gaps(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<String>> {
    let today = now.date_naive().to_string();
    let mut stmt = conn.prepare(
        "SELECT p.printing_id FROM printings p JOIN sets s ON s.set_code = p.set_code
          WHERE s.release_date > ?1",
    )?;
    let ids = stmt
        .query_map([today], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(ids)
}

pub fn run_report(conn: &Connection, now: DateTime<Utc>) -> Result<Report> {
    let mut all = stored(conn)?;
    all.extend(missing_price_days(conn, now)?);
    all.extend(coverage_drops(conn)?);
    all.extend(missing_sealed_images(conn)?);
    all.extend(numbering(conn)?);
    all.extend(datalint(conn)?);
    all.extend(missing_structured_text(conn)?);
    all.extend(suspect_rules_text(conn)?);
    all.extend(keyword_number_missing(conn)?);
    all.extend(keyword_missing_from_keywords(conn)?);
    all.extend(keyword_misclassified(conn)?);
    all.extend(mechanic_unmatched(conn)?);
    all.extend(wrong_group(conn)?);
    all.extend(image_wrong_card(conn)?);
    all.extend(variant_image_shared(conn)?);
    let latest: Option<String> = conn.query_row(
        "SELECT MAX(observed_on) FROM prices WHERE source = 'tcgcsv'",
        [],
        |r| r.get(0),
    )?;
    if let Some(day) = latest {
        let c = coverage(conn, &day)?;
        if c.ratio() < MIN_COVERAGE {
            all.push(AlarmRecord::alarm(
                "low_price_coverage",
                day.clone(),
                format!("{} of {} printings priced on {day}", c.priced, c.eligible),
            ));
        }
    }
    let table = accepted_table();
    let unreleased = unreleased_set_gaps(conn, now)?;
    let mut report = Report::default();
    for a in all {
        if a.severity != "alarm" {
            report.info.push(a);
        } else if let Some((_, _, why)) = table
            .iter()
            .find(|(k, key, _)| *k == a.kind && *key == a.key)
        {
            report.accepted.push((a, why.clone()));
        } else if a.kind == "official_without_tcgcsv" && unreleased.contains(&a.key) {
            report.accepted.push((
                a,
                "its set is not released yet; tcgcsv lists a set once it is".into(),
            ));
        } else {
            report.open.push(a);
        }
    }
    Ok(report)
}

/// The alarms that fail the unit.
pub fn run(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<AlarmRecord>> {
    Ok(run_report(conn, now)?.open)
}
