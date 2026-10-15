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

pub fn run(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<AlarmRecord>> {
    let mut out = stored(conn)?;
    out.extend(missing_price_days(conn, now)?);
    out.extend(coverage_drops(conn)?);
    let latest: Option<String> = conn.query_row(
        "SELECT MAX(observed_on) FROM prices WHERE source = 'tcgcsv'",
        [],
        |r| r.get(0),
    )?;
    if let Some(day) = latest {
        let c = coverage(conn, &day)?;
        if c.ratio() < MIN_COVERAGE {
            out.push(AlarmRecord::alarm(
                "low_price_coverage",
                day.clone(),
                format!("{} of {} printings priced on {day}", c.priced, c.eligible),
            ));
        }
    }
    Ok(out)
}
