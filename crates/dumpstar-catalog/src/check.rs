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

pub fn run(conn: &Connection, now: DateTime<Utc>) -> Result<Vec<AlarmRecord>> {
    let mut out = stored(conn)?;
    out.extend(missing_price_days(conn, now)?);
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
