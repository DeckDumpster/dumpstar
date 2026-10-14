//! Collection value history (dumpstar-e1vo).
//!
//! The `collection_value_snapshot` table (schema_user.sql) records the owned
//! collection's total market value, cost basis, and card count on each date,
//! along four dimensions: the loose cards (`all`), per set (`set`), per
//! binder (`binder`), and the sealed product (`sealed`). This module owns the
//! three operations over it:
//!
//! - [`snapshot_today`] computes and upserts *today's* rows.
//! - [`backfill`] reconstructs history from `shared.prices` × each copy's
//!   `acquired_at` / `status_log`, so the chart has a past even though the
//!   snapshot table starts empty.
//!
//! Both price a copy with the **same** rule, from the one macro in
//! [`crate::prices`] — the newest catalog `prices.market`, else (for a printing
//! the tenant invented) the tenant's `manual_prices` — differing only in the
//! feed relation and the manual arm's cutoff (see [`OWNED_TODAY_SQL`] and
//! [`OWNED_ASOF_SQL`]). They used not to: backfill
//! carried its own query with arm 1 and nothing else, so re-running it silently
//! rewrote every historical point *without* the curated and hand-entered
//! prices today's point has (pd-3lg8).
//! - [`value_history`] reads the table back for the API.
//!
//! VALUE model: for every OWNED copy, `market_price × conditionMultiplier`.
//! The multiplier is data — the collection's own `conditions` table (seeded
//! from `data/conditions.json` on open; pd-s4c2 moved it out of the catalog,
//! so this join no longer crosses the ATTACH boundary), defaulted to `1.0`
//! for an unknown condition, the
//! same defensive default the frontend uses. Cost basis is the sum of owned
//! copies' `purchase_price`; card count is the number of owned copies.
//!
//! ## Sealed is its own series, and `all` still means loose cards
//!
//! pd-bbv7. A collection's worth is two halves — loose cards and sealed
//! product — and reporting only the first under-reports it. They are two
//! *dimensions* rather than one blended number, because the two are priced
//! from different feeds against different keys and a reader is entitled to
//! know which half moved.
//!
//! `all` therefore keeps meaning exactly what every row already written under
//! it means: the loose cards. Widening it would silently restate months of
//! history. There is deliberately **no stored combined total** either — a
//! stored total can disagree with its parts, so the API sums the two series at
//! read time.
//!
//! The sealed row's shape is not the cards row's shape, and each difference is
//! a fact about sealed product rather than a convenience:
//!
//! * `card_count` is **units**, `SUM(quantity)` — one `sealed_collection` row
//!   is a lot of N identical boxes, where one `collection` row is one physical
//!   card. Counting rows would report 46 where the tenant owns 140.
//! * `cost_basis` is `SUM(purchase_price × quantity)`: `purchase_price` on a
//!   lot is per unit (it is Collectr's "Average Cost Paid", and
//!   `collectr_export` writes it back out beside `Quantity`).
//! * there is **no condition multiplier**. A sealed lot carries a `condition`,
//!   but nothing prices a box off it — `/sealed` does not, and inventing a
//!   multiplier here would make the chart disagree with the page.
//! * a lot whose product has no price is **skipped and counted**: `SUM` passes
//!   over the NULL while `SUM(quantity)` still counts the units. That is the
//!   same treatment an unpriced card already gets, and it is the point — a
//!   zero is indistinguishable from "worthless" on a chart.
//!
//! The sealed row is written only when the tenant owns at least one sealed
//! lot, exactly as a `set` or `binder` bucket exists only if something is in
//! it. A collection with no sealed product is left with the rows it has always
//! had.

use rusqlite::{Connection, params};

use crate::error::Result;

/// One point on a value series — the collection's value on a single date.
/// FROZEN API contract (dumpstar-e1vo.1): the chart frontend (e1vo.2) is
/// built against these exact field names.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ValuePoint {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub market_value: f64,
    pub cost_basis: f64,
    #[ts(type = "number")]
    pub card_count: i64,
}

/// A single value line: the whole collection, one set, or one binder.
/// FROZEN API contract (dumpstar-e1vo.1).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ValueSeries {
    /// `None` for the `all` dimension; otherwise the set_code or the
    /// binder-id-as-string that identifies this bucket.
    pub bucket: Option<String>,
    /// Human label — set name / binder name; `None` for `all`.
    pub label: Option<String>,
    pub points: Vec<ValuePoint>,
}

/// The per-copy "owned collection today" projection: one row per owned copy,
/// carrying its set, binder, purchase price, condition multiplier, and the
/// current market price (the feed → manual_prices, same COALESCE as the
/// collection page). Built into a TEMP TABLE so the three per-dimension
/// aggregates each scan it once.
const OWNED_TODAY_SQL: &str = concat!(
    "\
    CREATE TEMP TABLE _snap_owned AS \
    SELECT c.id, \
           c.purchase_price, \
           c.binder_id, \
           p.set_code, \
           COALESCE(cond.multiplier, 1.0) AS mult, \
           ",
    crate::market_price_expr!(),
    " AS market_price \
      FROM collection c \
      JOIN ",
    crate::printing_rel!(),
    " p ON c.printing_id = p.printing_id \
      LEFT JOIN conditions cond ON cond.name = c.condition \
     WHERE c.status = 'owned';"
);

/// The same per-copy projection as [`OWNED_TODAY_SQL`], for the collection as
/// it stood on a date D bound to `?1`: the copies owned on D, at the price
/// they carried on D. `_prices_asof` (staged per date by
/// [`backfill_one_date`]) stands in for `prices`, and the manual arm is cut off
/// at D — every other arm is the *same text*, because it is the same macro.
const OWNED_ASOF_SQL: &str = concat!(
    "\
    CREATE TEMP TABLE _snap_owned AS \
    SELECT c.id, \
           c.purchase_price, \
           c.binder_id, \
           p.set_code, \
           COALESCE(cond.multiplier, 1.0) AS mult, \
           ",
    crate::market_price_expr_asof!("?1"),
    " AS market_price \
      FROM collection c \
      JOIN ",
    crate::printing_rel!(),
    " p ON c.printing_id = p.printing_id \
      LEFT JOIN conditions cond ON cond.name = c.condition \
     WHERE date(c.acquired_at) <= ?1 \
       AND COALESCE( \
             (SELECT sl.to_status FROM status_log sl \
               WHERE sl.collection_id = c.id \
                 AND date(sl.changed_at) <= ?1 \
               ORDER BY sl.changed_at DESC, sl.id DESC LIMIT 1), \
             'owned') = 'owned';"
);

/// The per-lot "owned sealed product today" projection: one row per owned
/// `sealed_collection` lot, carrying its quantity, per-unit purchase price and
/// current market price. Staged into a TEMP TABLE beside `_snap_owned` so
/// [`insert_dimensions`] can aggregate both without either query knowing about
/// the other.
///
/// It joins **nothing**. The `sealed` dimension is one bucket, so it needs no
/// catalog attribute at all — and the join it is therefore not tempted to make
/// is the one that would quietly empty it: sealed product ids are not in
/// `tcgcsv_products` (that table holds single-card products), so
/// `JOIN tcgcsv_products` drops every sealed row and reports a collection with
/// no sealed product in it. Should a by-set sealed breakdown ever want a set
/// code, it comes from `sealed_products`, which is where a sealed product is
/// catalogued.
const OWNED_SEALED_TODAY_SQL: &str = concat!(
    "\
    CREATE TEMP TABLE _snap_sealed AS \
    SELECT sc.id, \
           sc.quantity, \
           sc.purchase_price, \
           ",
    crate::sealed_market_price_expr!(),
    " AS market_price \
      FROM sealed_collection sc \
     WHERE sc.status = 'owned';"
);

/// The same per-lot projection for a date D bound to `?1`, over the
/// `_sealed_prices_asof` relation [`backfill_one_date`] stages.
///
/// "Owned on D" is weaker here than it is for cards, and deliberately so:
/// `status_log` records transitions for `collection` rows only, so a lot's
/// **current** status is all there is to go on — the same accepted
/// approximation the card path already makes for condition. A lot acquired
/// after D is excluded; a lot sold since is absent from every reconstructed
/// day rather than from the days after the sale. Recording it would take a
/// status log for sealed, which is a change to the collection and not to this
/// reconstruction.
const OWNED_SEALED_ASOF_SQL: &str = concat!(
    "\
    CREATE TEMP TABLE _snap_sealed AS \
    SELECT sc.id, \
           sc.quantity, \
           sc.purchase_price, \
           ",
    crate::sealed_market_price_expr_asof!(),
    " AS market_price \
      FROM sealed_collection sc \
     WHERE sc.status = 'owned' \
       AND date(sc.added_at) <= ?1;"
);

/// Compute and upsert today's value rows for every dimension. `date` is
/// the `YYYY-MM-DD` snapshot key (the caller passes today). Idempotent: a
/// full delete-then-insert per `(date, dimension)`, so re-running replaces the
/// day's rows rather than duplicating them (a plain upsert can't, because the
/// `all` bucket is NULL and SQLite treats NULLs as distinct in the PK).
///
/// Returns the number of snapshot rows written.
pub fn snapshot_today(conn: &mut Connection, date: &str) -> Result<usize> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM collection_value_snapshot WHERE date = ?1",
        params![date],
    )?;
    tx.execute_batch("DROP TABLE IF EXISTS _snap_owned; DROP TABLE IF EXISTS _snap_sealed;")?;
    tx.execute_batch(OWNED_TODAY_SQL)?;
    tx.execute_batch(OWNED_SEALED_TODAY_SQL)?;

    let written = insert_dimensions(&tx, date)?;

    tx.execute_batch("DROP TABLE IF EXISTS _snap_owned; DROP TABLE IF EXISTS _snap_sealed;")?;
    tx.commit()?;
    Ok(written)
}

/// Insert the `all` / `set` / `binder` / `sealed` aggregate rows for `date`
/// from the current `_snap_owned` and `_snap_sealed` TEMP TABLEs. Shared by
/// [`snapshot_today`] and the per-date step of [`backfill`], which both stage
/// owned copies (with a `market_price` + `mult` column) into `_snap_owned` and
/// owned sealed lots (with `quantity`, `purchase_price` and `market_price`)
/// into `_snap_sealed` first.
///
/// The three card dimensions are computed from `_snap_owned` alone and the
/// sealed one from `_snap_sealed` alone. Nothing here reads both, which is
/// what makes "`all` still means loose cards" a property of the code rather
/// than of a filter somebody has to keep right.
fn insert_dimensions(tx: &rusqlite::Transaction, date: &str) -> Result<usize> {
    let mut written = 0usize;
    written += tx.execute(
        "INSERT INTO collection_value_snapshot \
             (date, dimension, bucket, market_value, cost_basis, card_count) \
         SELECT ?1, 'all', NULL, \
                COALESCE(SUM(market_price * mult), 0.0), \
                COALESCE(SUM(purchase_price), 0.0), \
                COUNT(*) \
           FROM _snap_owned",
        params![date],
    )?;
    written += tx.execute(
        "INSERT INTO collection_value_snapshot \
             (date, dimension, bucket, market_value, cost_basis, card_count) \
         SELECT ?1, 'set', set_code, \
                COALESCE(SUM(market_price * mult), 0.0), \
                COALESCE(SUM(purchase_price), 0.0), \
                COUNT(*) \
           FROM _snap_owned \
          GROUP BY set_code",
        params![date],
    )?;
    written += tx.execute(
        "INSERT INTO collection_value_snapshot \
             (date, dimension, bucket, market_value, cost_basis, card_count) \
         SELECT ?1, 'binder', CAST(binder_id AS TEXT), \
                COALESCE(SUM(market_price * mult), 0.0), \
                COALESCE(SUM(purchase_price), 0.0), \
                COUNT(*) \
           FROM _snap_owned \
          WHERE binder_id IS NOT NULL \
          GROUP BY binder_id",
        params![date],
    )?;
    // Sealed. `SUM(quantity)` because the count is UNITS, `× quantity` on both
    // money columns because a lot's prices are per unit, and no `mult` because
    // nothing prices a box off its condition. `HAVING COUNT(*) > 0` is what
    // keeps a collection that owns no sealed product exactly as it was: the
    // bucket exists only when something is in it, the same rule `set` and
    // `binder` follow.
    written += tx.execute(
        "INSERT INTO collection_value_snapshot \
             (date, dimension, bucket, market_value, cost_basis, card_count) \
         SELECT ?1, 'sealed', NULL, \
                COALESCE(SUM(market_price * quantity), 0.0), \
                COALESCE(SUM(purchase_price * quantity), 0.0), \
                COALESCE(SUM(quantity), 0) \
           FROM _snap_sealed \
          HAVING COUNT(*) > 0",
        params![date],
    )?;
    Ok(written)
}

/// Reconstruct historical value rows from `shared.prices` /
/// `shared.sealed_prices` and each copy's acquisition + status history. For
/// every distinct price-observation date D at or after the earliest
/// acquisition, records the value of the collection as it stood on D:
///
/// - **owned on D**: a copy with `date(acquired_at) <= D` whose status as of D
///   is `owned`. Status-as-of-D is the `to_status` of the latest `status_log`
///   transition with `date(changed_at) <= D`; if there is none (e.g. the only
///   log row is a backdated acquisition recorded later), the copy is treated
///   as owned since acquisition.
/// - **price as of D**: [`crate::prices`]'s two arms, resolved at D — the
///   latest `prices.market` with `observed_on <= D` for the copy's printing,
///   then the tenant's own `manual_prices` observed on or before D (user
///   printings only). Arm 1 is the only one whose *shape* changes with the
///   date; the expression itself is the same macro `snapshot_today` spends, so
///   the two paths cannot drift (pd-3lg8).
/// - **condition**: the copy's *current* condition — the accepted
///   approximation, since there is no condition history.
/// - **sealed on D**: a lot with `date(added_at) <= D` that is `owned` today,
///   at the newest `sealed_prices` observation on or before D. See
///   [`OWNED_SEALED_ASOF_SQL`] for why that filter is weaker than the card
///   one.
///
/// Idempotent: each date's rows are deleted before being re-inserted.
///
/// PERF: the trap is O(dates × copies × price-lookup). We avoid it two ways —
/// (1) an `_owned_printings` TEMP TABLE (built once) restricts the per-date
/// price materialization to the handful of printings the collection has ever
/// held, so `_prices_asof` is a small join, not a scan of the whole
/// multi-million-row `prices` table; (2) `_prices_asof` is built once per date
/// and joined to owned copies, never a correlated per-copy subquery. Each date
/// is its own transaction (interrupt-safe) with a flushed progress line.
///
/// Returns the total number of snapshot rows written.
pub fn backfill(conn: &mut Connection) -> Result<usize> {
    use std::io::Write;

    // Printings the collection has ever held. Restricting the per-date price
    // materialization to these is what keeps backfill cheap.
    conn.execute_batch(
        "DROP TABLE IF EXISTS _owned_printings; \
         CREATE TEMP TABLE _owned_printings AS \
         SELECT DISTINCT printing_id FROM collection; \
         CREATE INDEX _owned_printings_idx ON _owned_printings(printing_id);",
    )?;
    // The same restriction for the sealed feed, and for the same reason: a
    // per-date materialization of `sealed_prices` is only cheap if it is
    // scoped to the handful of products the tenant has ever held.
    conn.execute_batch(
        "DROP TABLE IF EXISTS _owned_sealed_products; \
         CREATE TEMP TABLE _owned_sealed_products AS \
         SELECT DISTINCT product_id FROM sealed_collection; \
         CREATE INDEX _owned_sealed_products_idx \
           ON _owned_sealed_products(product_id);",
    )?;

    // Earliest acquisition date — no point recording dates before the
    // collection held anything (those days would all be value 0). Both
    // halves count: a tenant who owns sealed product and no loose cards has a
    // value history, and reading `collection` alone would report that they
    // have none.
    let earliest: Option<String> = conn.query_row(
        "SELECT MIN(d) FROM ( \
             SELECT MIN(date(acquired_at)) AS d FROM collection \
             UNION ALL \
             SELECT MIN(date(added_at))    AS d FROM sealed_collection)",
        [],
        |r| r.get(0),
    )?;
    let Some(earliest) = earliest else {
        println!("Backfill: collection is empty — nothing to reconstruct.");
        return Ok(0);
    };

    // Distinct price-observation dates at or after the first acquisition,
    // oldest first — from BOTH feeds. A day the sealed feed moved is a day
    // the collection's value moved, and it would otherwise be reconstructed
    // only if the card feed happened to be quoted the same day. (It is, every
    // night; the union is here so that staying true is not a coincidence.)
    let dates: Vec<String> = {
        let mut stmt = conn.prepare(
            "SELECT DISTINCT observed_at FROM ( \
                 SELECT observed_on AS observed_at FROM prices WHERE market IS NOT NULL \
                 UNION \
                 SELECT observed_at FROM sealed_prices \
               ) \
              WHERE observed_at >= ?1 \
              ORDER BY observed_at",
        )?;
        let rows = stmt.query_map(params![earliest], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    println!(
        "Backfill: {} distinct price date(s) from {} onward.",
        dates.len(),
        earliest
    );
    std::io::stdout().flush().ok();

    let mut written = 0usize;
    for (i, date) in dates.iter().enumerate() {
        written += backfill_one_date(conn, date)?;
        if (i + 1) % 25 == 0 || i + 1 == dates.len() {
            println!(
                "  {}/{} dates ({} rows so far)",
                i + 1,
                dates.len(),
                written
            );
            std::io::stdout().flush().ok();
        }
    }

    conn.execute_batch(
        "DROP TABLE IF EXISTS _owned_printings; DROP TABLE IF EXISTS _owned_sealed_products;",
    )?;
    println!(
        "Backfill: wrote {written} snapshot rows across {} dates.",
        dates.len()
    );
    Ok(written)
}

/// Reconstruct and write the three dimension rows for one date D. Assumes the
/// `_owned_printings` TEMP TABLE already exists (built once by [`backfill`]).
fn backfill_one_date(conn: &mut Connection, date: &str) -> Result<usize> {
    let tx = conn.transaction()?;
    tx.execute(
        "DELETE FROM collection_value_snapshot WHERE date = ?1",
        params![date],
    )?;

    // Latest market price <= D per owned printing. Built once per date; the
    // owned-printings restriction keeps it small.
    tx.execute_batch(
        "DROP TABLE IF EXISTS _prices_asof; DROP TABLE IF EXISTS _snap_owned; \
         DROP TABLE IF EXISTS _sealed_prices_asof; DROP TABLE IF EXISTS _snap_sealed;",
    )?;
    tx.execute(
        "CREATE TEMP TABLE _prices_asof AS \
         SELECT pr.printing_id, pr.market \
           FROM prices pr \
           JOIN ( \
                  SELECT printing_id, MAX(observed_on) AS mo \
                    FROM prices \
                   WHERE market IS NOT NULL AND observed_on <= ?1 \
                     AND printing_id IN (SELECT printing_id FROM _owned_printings) \
                   GROUP BY printing_id \
                ) m \
             ON m.printing_id = pr.printing_id AND m.mo = pr.observed_on \
          WHERE pr.market IS NOT NULL",
        params![date],
    )?;
    tx.execute_batch("CREATE INDEX _prices_asof_idx ON _prices_asof(printing_id);")?;

    // The sealed feed as of D: the newest observation at or before D per
    // owned product, whole — market and mid come off the SAME row, because
    // `COALESCE(market, mid)` is a choice between two quotes of one day and
    // not a search back through the series for whichever exists.
    tx.execute(
        "CREATE TEMP TABLE _sealed_prices_asof AS \
         SELECT sp.tcgplayer_product_id, sp.market_price, sp.mid_price \
           FROM sealed_prices sp \
           JOIN ( \
                  SELECT tcgplayer_product_id, MAX(observed_at) AS mo \
                    FROM sealed_prices \
                   WHERE observed_at <= ?1 \
                     AND tcgplayer_product_id IN \
                         (SELECT product_id FROM _owned_sealed_products) \
                   GROUP BY tcgplayer_product_id \
                ) m \
             ON m.tcgplayer_product_id = sp.tcgplayer_product_id \
            AND m.mo = sp.observed_at",
        params![date],
    )?;
    tx.execute_batch(
        "CREATE INDEX _sealed_prices_asof_idx \
           ON _sealed_prices_asof(tcgplayer_product_id);",
    )?;

    // Copies owned on D, priced as of D. Same _snap_owned shape
    // insert_dimensions expects (market_price + mult columns), and the same
    // rule snapshot_today spends — spelled once, in
    // `market_price_expr_from!`, with `_prices_asof` standing in for
    // `prices` and the manual arm cut off at D.
    tx.execute(OWNED_ASOF_SQL, params![date])?;
    tx.execute(OWNED_SEALED_ASOF_SQL, params![date])?;

    let written = insert_dimensions(&tx, date)?;

    tx.execute_batch(
        "DROP TABLE IF EXISTS _prices_asof; DROP TABLE IF EXISTS _snap_owned; \
         DROP TABLE IF EXISTS _sealed_prices_asof; DROP TABLE IF EXISTS _snap_sealed;",
    )?;
    tx.commit()?;
    Ok(written)
}

/// The `bucket` the sealed series is returned under by [`value_history`], and
/// the one value of `bucket` on the `all` dimension that is not `None`.
///
/// A constant because the frontend matches on it: the two series the `all`
/// dimension answers with are told apart by this, never by their order.
pub const SEALED_BUCKET: &str = "sealed";

/// Read the value history for the API. `dimension` is `all`, `set`, or
/// `binder` (an unknown value is the caller's responsibility to default).
///
/// - `all` → the collection's two priced halves, cards first: the loose-card
///   series (`bucket`/`label` both `None` — unchanged, and always present
///   even when empty) and, when the tenant has ever owned sealed product, a
///   second series at [`SEALED_BUCKET`]. Points sorted by date ascending.
/// - `set` / `binder` → one series per bucket, points sorted by date; series
///   sorted by their latest (most recent date) market value, descending. The
///   label is the set name / binder name.
///
/// **There is no combined total here, deliberately** (pd-bbv7). A stored or
/// server-side total is a third number that can disagree with the two it is
/// made of; the caller adds the halves it drew, on the date it drew them.
pub fn value_history(conn: &Connection, dimension: &str) -> Result<Vec<ValueSeries>> {
    if dimension == "all" {
        let points_of = |dim: &str| -> Result<Vec<ValuePoint>> {
            let mut stmt = conn.prepare(
                "SELECT date, market_value, cost_basis, card_count \
                   FROM collection_value_snapshot \
                  WHERE dimension = ?1 \
                  ORDER BY date",
            )?;
            let points = stmt
                .query_map(params![dim], |r| {
                    Ok(ValuePoint {
                        date: r.get(0)?,
                        market_value: r.get(1)?,
                        cost_basis: r.get(2)?,
                        card_count: r.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(points)
        };

        // Always exactly one series for the cards, even when empty — the
        // frontend has read `series[0]` as "the collection" since e1vo.
        let mut series = vec![ValueSeries {
            bucket: None,
            label: None,
            points: points_of("all")?,
        }];
        // And the sealed half beside it, only when there is one. An empty
        // series would draw a flat zero line for every tenant who owns no
        // sealed product, which says "worth nothing" where the truth is
        // "none held".
        let sealed = points_of(SEALED_BUCKET)?;
        if !sealed.is_empty() {
            series.push(ValueSeries {
                bucket: Some(SEALED_BUCKET.to_string()),
                label: Some("Sealed".to_string()),
                points: sealed,
            });
        }
        return Ok(series);
    }

    // set / binder: bucket + label come from the catalog / user tables.
    let sql = match dimension {
        "binder" => {
            "SELECT cvs.bucket, b.name, cvs.date, \
                    cvs.market_value, cvs.cost_basis, cvs.card_count \
               FROM collection_value_snapshot cvs \
               LEFT JOIN binders b ON b.id = CAST(cvs.bucket AS INTEGER) \
              WHERE cvs.dimension = 'binder' \
              ORDER BY cvs.bucket, cvs.date"
        }
        // 'set' (and any caller-normalized default that reaches here).
        _ => {
            "SELECT cvs.bucket, s.name, cvs.date, \
                    cvs.market_value, cvs.cost_basis, cvs.card_count \
               FROM collection_value_snapshot cvs \
               LEFT JOIN sets s ON s.set_code = cvs.bucket \
              WHERE cvs.dimension = 'set' \
              ORDER BY cvs.bucket, cvs.date"
        }
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        let bucket: Option<String> = r.get(0)?;
        let label: Option<String> = r.get(1)?;
        let point = ValuePoint {
            date: r.get(2)?,
            market_value: r.get(3)?,
            cost_basis: r.get(4)?,
            card_count: r.get(5)?,
        };
        Ok((bucket, label, point))
    })?;

    // Group consecutive rows by bucket (the query is ordered by bucket, date).
    let mut series: Vec<ValueSeries> = Vec::new();
    for row in rows {
        let (bucket, label, point) = row?;
        match series.last_mut() {
            Some(s) if s.bucket == bucket => s.points.push(point),
            _ => series.push(ValueSeries {
                bucket,
                label,
                points: vec![point],
            }),
        }
    }

    // Sort series by their latest (most recent date) market value, desc. The
    // points within each series are already date-ascending, so the last point
    // is the latest.
    series.sort_by(|a, b| {
        let av = a.points.last().map(|p| p.market_value).unwrap_or(0.0);
        let bv = b.points.last().map(|p| p.market_value).unwrap_or(0.0);
        bv.partial_cmp(&av).unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(series)
}

/// One tenant a fleet run is asked to value: the handle for the report, the
/// file for the work.
#[derive(Debug, Clone)]
pub struct FleetTarget {
    pub handle: String,
    pub path: std::path::PathBuf,
}

/// What a fleet run did, per tenant.
#[derive(Debug, Default)]
pub struct FleetReport {
    /// `(handle, snapshot rows written)` for every tenant that was valued.
    pub valued: Vec<(String, usize)>,
    /// `(handle, why)` for every tenant that was skipped.
    pub skipped: Vec<(String, String)>,
}

/// Write `date`'s value rows into every target's own collection, valuing each
/// from the shared catalog at `shared`.
///
/// One tenant's file being absent, locked, mid-restore or not a database
/// skips **that tenant** and is named in the report: abandoning the rest
/// would leave the fleet half-snapshotted with nothing saying which half, and
/// stopping silently is how a chart grows a gap nobody sees. Each tenant is
/// opened and written on its own connection, so no tenant's rows are ever
/// computed from, or written through, another's.
pub fn snapshot_fleet(
    shared: &std::path::Path,
    targets: &[FleetTarget],
    date: &str,
) -> FleetReport {
    let mut report = FleetReport::default();
    for t in targets {
        let outcome = if !t.path.exists() {
            Err(format!("{} is not on this box", t.path.display()))
        } else {
            crate::connect_user(&t.path, shared)
                .and_then(|mut conn| snapshot_today(&mut conn, date))
                .map_err(|e| e.to_string())
        };
        match outcome {
            Ok(rows) => report.valued.push((t.handle.clone(), rows)),
            Err(why) => report.skipped.push((t.handle.clone(), why)),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use crate::{connect_user, open_shared};
    use rusqlite::Connection;

    /// Build a shared catalog with one set, two cards, and printings, then
    /// return a user connection attached to it.
    ///
    /// Catalog:
    ///   set SOR "Spark of Rebellion"
    ///   card SOR-1 "Alpha" -> printing SOR-1-normal (Standard Normal)
    ///   card SOR-2 "Beta"  -> printing SOR-2-foil   (Standard Foil)
    fn fixture() -> (tempfile::TempDir, Connection, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::simple(&c, "SOR", "1", "Alpha");
            testkit::card(&c, "SOR-2", "Beta");
            testkit::printing(&c, "SOR-2-foil", "SOR-2", "SOR", "2", "Standard", "Foil");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn, shared)
    }

    fn add_copy(conn: &mut Connection, printing: &str, condition: &str, price: f64) -> i64 {
        conn.execute(
            "INSERT INTO collection (printing_id, condition, purchase_price, acquired_at, source, status) \
             VALUES (?1, ?2, ?3, ?4, 'manual_id', 'owned')",
            params![printing, condition, price, "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO status_log (collection_id, from_status, to_status, changed_at, note) \
             VALUES (?1, NULL, 'owned', ?2, 'added')",
            params![id, "2026-01-01T00:00:00Z"],
        )
        .unwrap();
        id
    }

    /// Append a market-price observation. `prices` is a shared-catalog table
    /// (a read-only view on the user connection), so the write goes through a
    /// fresh read-write handle on the shared DB. The user connection reads the
    /// committed rows back through its attachment.
    fn set_price(shared: &std::path::Path, printing_id: &str, price: f64, observed: &str) {
        let c = open_shared(shared).unwrap();
        testkit::price(&c, printing_id, observed, price);
    }

    /// Catalogue a sealed product and quote it, on the shared DB. `market` is
    /// deliberately optional: TCGCSV quotes plenty of sealed products a mid
    /// and no market, which is why the price rule coalesces the two.
    fn set_sealed_price(
        shared: &std::path::Path,
        product: i64,
        market: Option<f64>,
        mid: Option<f64>,
        observed: &str,
    ) {
        let c = open_shared(shared).unwrap();
        c.execute(
            "INSERT OR IGNORE INTO sealed_products (product_id, name, category, fetched_at) \
             VALUES (?1, 'A Booster Display', 'booster_display', '2026-01-01T00:00:00Z')",
            params![product],
        )
        .unwrap();
        c.execute(
            "INSERT INTO sealed_prices \
               (tcgplayer_product_id, market_price, mid_price, observed_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![product, market, mid, observed],
        )
        .unwrap();
    }

    /// Add a sealed lot to the collection. `quantity` is the whole point: one
    /// row is N identical boxes.
    fn add_sealed(conn: &Connection, product: i64, quantity: i64, unit_cost: Option<f64>) {
        conn.execute(
            "INSERT INTO sealed_collection \
               (product_id, quantity, purchase_price, added_at, status) \
             VALUES (?1, ?2, ?3, '2026-01-01T00:00:00Z', 'owned')",
            params![product, quantity, unit_cost],
        )
        .unwrap();
    }

    /// One dimension's row for a date, or `None` if it was not written.
    fn dimension_row(conn: &Connection, date: &str, dimension: &str) -> Option<(f64, f64, i64)> {
        conn.query_row(
            "SELECT market_value, cost_basis, card_count \
               FROM collection_value_snapshot \
              WHERE date = ?1 AND dimension = ?2 AND bucket IS NULL",
            params![date, dimension],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()
    }

    fn point_for(
        conn: &Connection,
        dimension: &str,
        bucket: Option<&str>,
        date: &str,
    ) -> ValuePoint {
        let series = value_history(conn, dimension).unwrap();
        let s = series
            .iter()
            .find(|s| s.bucket.as_deref() == bucket)
            .unwrap_or_else(|| panic!("no series for bucket {bucket:?}"));
        s.points
            .iter()
            .find(|p| p.date == date)
            .unwrap_or_else(|| panic!("no point on {date}"))
            .clone()
    }

    #[test]
    fn snapshot_today_computes_value_cost_and_count() {
        let (_d, mut conn, shared) = fixture();
        // Alpha (SOR-1-normal) market 10.00; Beta (SOR-2-foil) market 40.00.
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-01");
        set_price(&shared, "SOR-2-foil", 40.00, "2026-06-01");

        // Two NM Alphas (10.00 each), one Lightly Played Beta (40 * 0.85 = 34).
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 3.00);
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 4.00);
        add_copy(&mut conn, "SOR-2-foil", "Lightly Played", 20.00);

        let n = snapshot_today(&mut conn, "2026-06-02").unwrap();
        // 1 'all' + 1 'set' bucket (both cards live in SOR) + 0 'binder' = 2 rows.
        assert_eq!(n, 2);

        let all = point_for(&conn, "all", None, "2026-06-02");
        // 10 + 10 + 34 = 54.00
        assert!(
            (all.market_value - 54.00).abs() < 1e-9,
            "value {}",
            all.market_value
        );
        // cost basis 3 + 4 + 20 = 27
        assert!(
            (all.cost_basis - 27.00).abs() < 1e-9,
            "cost {}",
            all.cost_basis
        );
        assert_eq!(all.card_count, 3);

        // Per-set: SOR holds all three copies.
        let sor = point_for(&conn, "set", Some("SOR"), "2026-06-02");
        assert!((sor.market_value - 54.00).abs() < 1e-9);
        assert_eq!(sor.card_count, 3);
    }

    #[test]
    fn snapshot_today_is_idempotent() {
        let (_d, mut conn, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-01");
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 1.00);

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM collection_value_snapshot WHERE date = '2026-06-02'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2, "re-run replaces, not duplicates (1 all + 1 set)");
    }

    #[test]
    fn snapshot_excludes_non_owned_copies() {
        let (_d, mut conn, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-01");
        let sold = add_copy(&mut conn, "SOR-1-normal", "Near Mint", 1.00);
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 1.00);
        conn.execute(
            "UPDATE collection SET status = 'sold' WHERE id = ?1",
            params![sold],
        )
        .unwrap();

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let all = point_for(&conn, "all", None, "2026-06-02");
        assert_eq!(all.card_count, 1, "only the owned copy counts");
        assert!((all.market_value - 10.00).abs() < 1e-9);
    }

    #[test]
    fn snapshot_groups_by_binder() {
        let (_d, mut conn, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-01");
        set_price(&shared, "SOR-2-foil", 40.00, "2026-06-01");
        conn.execute(
            "INSERT INTO binders (id, name, created_at, updated_at) \
             VALUES (7, 'Fav', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let a = add_copy(&mut conn, "SOR-1-normal", "Near Mint", 1.00);
        add_copy(&mut conn, "SOR-2-foil", "Near Mint", 1.00); // unassigned
        conn.execute(
            "UPDATE collection SET binder_id = 7 WHERE id = ?1",
            params![a],
        )
        .unwrap();

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let series = value_history(&conn, "binder").unwrap();
        assert_eq!(series.len(), 1, "only the assigned binder gets a bucket");
        assert_eq!(series[0].bucket.as_deref(), Some("7"));
        assert_eq!(series[0].label.as_deref(), Some("Fav"));
        assert!((series[0].points[0].market_value - 10.00).abs() < 1e-9);
    }

    #[test]
    fn backfill_reconstructs_value_over_dates() {
        let (_d, mut conn, shared) = fixture();

        // Alpha bought 2026-03-01, Beta bought 2026-05-01 (backdated acquisitions).
        conn.execute(
            "INSERT INTO collection (printing_id, condition, purchase_price, acquired_at, source, status) \
             VALUES ('SOR-1-normal', 'Near Mint', 2.00, '2026-03-01T00:00:00Z', 'manual_id', 'owned')",
            [],
        )
        .unwrap();
        let alpha = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO status_log (collection_id, from_status, to_status, changed_at, note) \
             VALUES (?1, NULL, 'owned', '2026-06-30T00:00:00Z', 'added')",
            params![alpha],
        )
        .unwrap(); // status_log recorded LATE (after the price dates) — backfill must
        // fall back to 'owned since acquired_at'.
        conn.execute(
            "INSERT INTO collection (printing_id, condition, purchase_price, acquired_at, source, status) \
             VALUES ('SOR-2-foil', 'Near Mint', 30.00, '2026-05-01T00:00:00Z', 'manual_id', 'owned')",
            [],
        )
        .unwrap();
        let beta = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO status_log (collection_id, from_status, to_status, changed_at, note) \
             VALUES (?1, NULL, 'owned', '2026-06-30T00:00:00Z', 'added')",
            params![beta],
        )
        .unwrap();

        // Prices: Alpha rises 10 → 15; Beta priced at 40 from April.
        set_price(&shared, "SOR-1-normal", 10.00, "2026-04-01");
        set_price(&shared, "SOR-1-normal", 15.00, "2026-05-15");
        set_price(&shared, "SOR-2-foil", 40.00, "2026-04-01");

        let n = backfill(&mut conn).unwrap();
        assert!(n > 0);

        // 2026-04-01: only Alpha owned (Beta acquired 05-01). Value = 10.
        let d1 = point_for(&conn, "all", None, "2026-04-01");
        assert_eq!(d1.card_count, 1, "only Alpha owned on 04-01");
        assert!(
            (d1.market_value - 10.00).abs() < 1e-9,
            "value {}",
            d1.market_value
        );

        // 2026-05-15: Alpha (now 15) + Beta (40, latest <=D is the 04-01 obs) = 55.
        let d2 = point_for(&conn, "all", None, "2026-05-15");
        assert_eq!(d2.card_count, 2, "both owned by 05-15");
        assert!(
            (d2.market_value - 55.00).abs() < 1e-9,
            "value {}",
            d2.market_value
        );
        // cost basis grows to 2 + 30 = 32.
        assert!((d2.cost_basis - 32.00).abs() < 1e-9);
    }

    #[test]
    fn backfill_respects_sold_status_from_log() {
        let (_d, mut conn, shared) = fixture();
        conn.execute(
            "INSERT INTO collection (printing_id, condition, purchase_price, acquired_at, source, status) \
             VALUES ('SOR-1-normal', 'Near Mint', 2.00, '2026-03-01T00:00:00Z', 'manual_id', 'sold')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        // Owned from acquisition, then sold on 2026-05-01.
        conn.execute(
            "INSERT INTO status_log (collection_id, from_status, to_status, changed_at, note) \
             VALUES (?1, NULL, 'owned', '2026-03-01T00:00:00Z', 'added'), \
                    (?1, 'owned', 'sold', '2026-05-01T00:00:00Z', 'ebay')",
            params![id],
        )
        .unwrap();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-04-01");
        set_price(&shared, "SOR-1-normal", 12.00, "2026-06-01");

        backfill(&mut conn).unwrap();

        // 04-01: owned → counted.
        let d1 = point_for(&conn, "all", None, "2026-04-01");
        assert_eq!(d1.card_count, 1);
        assert!((d1.market_value - 10.00).abs() < 1e-9);

        // 06-01: sold before this date → not counted.
        let d2 = point_for(&conn, "all", None, "2026-06-01");
        assert_eq!(d2.card_count, 0, "sold copy drops out after the sale date");
        assert!((d2.market_value - 0.00).abs() < 1e-9);
    }

    /// Every snapshot row for `date`, in a stable order — what the two paths
    /// must agree on, column for column.
    fn rows_on(
        conn: &Connection,
        date: &str,
    ) -> Vec<(String, String, Option<String>, f64, f64, i64)> {
        let mut stmt = conn
            .prepare(
                "SELECT date, dimension, bucket, market_value, cost_basis, card_count \
                   FROM collection_value_snapshot \
                  WHERE date = ?1 \
                  ORDER BY dimension, bucket",
            )
            .unwrap();
        stmt.query_map(params![date], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
    }

    /// pd-3lg8: the live path and the backfill must be the SAME arithmetic.
    ///
    /// The collection holds one copy priced by each arm of the rule — the
    /// catalog feed, and a `manual_prices` row for a printing this tenant
    /// invented — plus a catalog printing the feed does not price at all,
    /// which a stray `manual_prices` row for it must NOT price (a gap in the
    /// feed is the catalog's to fill, not one tenant's). `snapshot_today` for
    /// D and `backfill` ending on D must write byte-identical rows for D.
    #[test]
    fn snapshot_today_and_backfill_agree_on_the_same_date() {
        let (_d, mut conn, shared) = fixture();

        // A catalog printing the feed does not price.
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "3", "Gamma");
        }

        // The tenant invented a printing of Alpha and hand-priced it.
        conn.execute(
            "INSERT INTO user_printings (printing_id, card_id, variant, created_at) \
             VALUES ('SOR-1-user-1', 'SOR-1', 'invented', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO manual_prices (printing_id, price, observed_at) \
             VALUES ('SOR-1-user-1', 7.50, '2026-05-20T00:00:00Z'), \
                    ('SOR-3-normal', 99.00, '2026-05-20T00:00:00Z')",
            [],
        )
        .unwrap();

        // The feed's subject, observed on the very day we snapshot — which is
        // also what puts D in backfill's date list.
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-02");

        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 3.00);
        add_copy(&mut conn, "SOR-3-normal", "Lightly Played", 40.00);
        add_copy(&mut conn, "SOR-1-user-1", "Near Mint", 2.00);

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let live = rows_on(&conn, "2026-06-02");

        // 10.00 + 7.50, and nothing for the unpriced catalog printing.
        assert_eq!(live.len(), 2, "1 'all' + 1 'set' row");
        assert!(
            (live[0].3 - 17.50).abs() < 1e-9,
            "live 'all' value {} — both arms contribute, the manual price of a \
             catalog printing does not",
            live[0].3
        );
        assert_eq!(live[0].5, 3, "the unpriced copy is still counted");

        backfill(&mut conn).unwrap();
        let reconstructed = rows_on(&conn, "2026-06-02");

        assert_eq!(
            reconstructed, live,
            "backfill must reconstruct the same day the live path computes"
        );
    }

    /// **The hard gate of pd-bbv7.** Sealed product must not move the cards.
    ///
    /// Stated the only way a test can state "before and after": the same
    /// collection, snapshotted with its sealed lots in place and again with
    /// them gone, and the `all` / `set` / `binder` rows required to be
    /// identical to the last bit. An accidental blend — a sealed lot joined
    /// into `_snap_owned`, a widened `all` — changes the first set of rows and
    /// not the second, and this is where that shows up.
    #[test]
    fn sealed_holdings_do_not_move_the_cards_dimensions() {
        let (_d, mut conn, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-02");
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 3.00);
        set_sealed_price(&shared, 7001, Some(120.00), Some(115.00), "2026-06-02");
        add_sealed(&conn, 7001, 4, Some(90.00));

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let with_sealed = rows_on(&conn, "2026-06-02");
        assert_eq!(
            dimension_row(&conn, "2026-06-02", "sealed"),
            Some((480.0, 360.0, 4)),
            "4 boxes at $120 = $480, at $90 each paid = $360, 4 UNITS"
        );

        // The same collection with no sealed product in it at all.
        conn.execute("DELETE FROM sealed_collection", []).unwrap();
        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let cards_only = rows_on(&conn, "2026-06-02");

        assert_eq!(
            with_sealed
                .iter()
                .filter(|r| r.1 != "sealed")
                .collect::<Vec<_>>(),
            cards_only.iter().collect::<Vec<_>>(),
            "owning sealed product changed a cards row — `all` has been blended"
        );
        assert!(
            cards_only.iter().all(|r| r.1 != "sealed"),
            "a collection with no sealed product must get no sealed row"
        );
    }

    /// A sealed lot whose product nobody quotes is **skipped and counted**:
    /// out of the money, still in the unit count. Valuing it at zero would be
    /// indistinguishable from a box that is worthless.
    #[test]
    fn an_unpriced_sealed_lot_is_skipped_and_still_counted() {
        let (_d, mut conn, shared) = fixture();
        set_sealed_price(&shared, 7001, Some(50.00), None, "2026-06-02");
        add_sealed(&conn, 7001, 2, Some(30.00));
        // Catalogued, never quoted.
        {
            let c = open_shared(&shared).unwrap();
            c.execute(
                "INSERT INTO sealed_products (product_id, name, category, fetched_at) \
                 VALUES (7002, 'An Unquoted Tin', 'tin', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }
        add_sealed(&conn, 7002, 3, Some(20.00));

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        assert_eq!(
            dimension_row(&conn, "2026-06-02", "sealed"),
            Some((100.0, 120.0, 5)),
            "the priced lot's $100 alone, both lots' cost, and all FIVE units"
        );
    }

    /// TCGCSV quotes many sealed products a mid and no market, so the rule is
    /// `COALESCE(market, mid)` — off ONE observation, never market from one
    /// day and mid from another.
    #[test]
    fn a_sealed_lot_falls_back_to_the_mid_price() {
        let (_d, mut conn, shared) = fixture();
        set_sealed_price(&shared, 7001, None, Some(45.00), "2026-06-02");
        add_sealed(&conn, 7001, 1, None);

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        assert_eq!(
            dimension_row(&conn, "2026-06-02", "sealed"),
            Some((45.0, 0.0, 1))
        );
    }

    /// pd-3lg8's rule, extended to the row pd-bbv7 adds: the live path and the
    /// backfill must be the same arithmetic on the sealed side too, or every
    /// historical sealed point disagrees with today's.
    #[test]
    fn snapshot_today_and_backfill_agree_on_the_sealed_row() {
        let (_d, mut conn, shared) = fixture();
        // Two sealed observations, so "as of D" has an older one to reject.
        set_sealed_price(&shared, 7001, Some(100.00), Some(99.00), "2026-06-01");
        set_sealed_price(&shared, 7001, Some(130.00), Some(128.00), "2026-06-02");
        add_sealed(&conn, 7001, 3, Some(80.00));
        // A card too, so backfill has a `collection` to walk and the two
        // dimensions are written by the same call.
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-02");
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 3.00);

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let live = rows_on(&conn, "2026-06-02");
        assert_eq!(
            dimension_row(&conn, "2026-06-02", "sealed"),
            Some((390.0, 240.0, 3)),
            "the newest quote, 3 × $130"
        );

        backfill(&mut conn).unwrap();
        assert_eq!(
            rows_on(&conn, "2026-06-02"),
            live,
            "backfill must reconstruct the same day the live path computes"
        );
        // And the older day is valued at the older quote, not today's.
        assert_eq!(
            dimension_row(&conn, "2026-06-01", "sealed"),
            Some((300.0, 240.0, 3)),
            "3 × $100 — the observation that stood on 2026-06-01"
        );
    }

    /// A tenant who owns sealed product and no loose cards still has a value
    /// history. Reading `collection` alone for the earliest date would report
    /// that they have none.
    #[test]
    fn a_sealed_only_collection_still_backfills() {
        let (_d, mut conn, shared) = fixture();
        set_sealed_price(&shared, 7001, Some(60.00), None, "2026-06-02");
        add_sealed(&conn, 7001, 2, Some(50.00));

        assert!(
            backfill(&mut conn).unwrap() > 0,
            "nothing was reconstructed"
        );
        assert_eq!(
            dimension_row(&conn, "2026-06-02", "sealed"),
            Some((120.0, 100.0, 2))
        );
    }

    /// The API's `all` dimension answers with the collection's two priced
    /// halves. The cards series stays first and keeps `bucket = None`, which
    /// is what every existing reader indexes.
    #[test]
    fn value_history_all_returns_the_sealed_series_beside_the_cards() {
        let (_d, mut conn, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.00, "2026-06-02");
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 3.00);

        // No sealed yet: exactly the one series this has always returned.
        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let series = value_history(&conn, "all").unwrap();
        assert_eq!(series.len(), 1);
        assert!(series[0].bucket.is_none());

        set_sealed_price(&shared, 7001, Some(25.00), None, "2026-06-02");
        add_sealed(&conn, 7001, 2, Some(20.00));
        snapshot_today(&mut conn, "2026-06-02").unwrap();

        let series = value_history(&conn, "all").unwrap();
        assert_eq!(series.len(), 2, "cards and sealed");
        assert!(
            series[0].bucket.is_none(),
            "the cards series is still first"
        );
        assert_eq!(series[0].points.last().unwrap().market_value, 10.0);
        assert_eq!(series[1].bucket.as_deref(), Some(SEALED_BUCKET));
        assert_eq!(series[1].label.as_deref(), Some("Sealed"));
        assert_eq!(series[1].points.last().unwrap().market_value, 50.0);
        // No combined total anywhere: the caller adds the halves it drew.
        assert!(
            series
                .iter()
                .all(|s| s.points.iter().all(|p| p.market_value != 60.0)),
            "a blended total has appeared in the response"
        );
    }

    #[test]
    fn value_history_all_returns_single_series_even_when_empty() {
        let (_d, conn, _shared) = fixture();
        let series = value_history(&conn, "all").unwrap();
        assert_eq!(series.len(), 1);
        assert!(series[0].bucket.is_none());
        assert!(series[0].label.is_none());
        assert!(series[0].points.is_empty());
    }

    #[test]
    fn value_history_sets_sorted_by_latest_value_desc() {
        // Two sets; the more valuable one must come first.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Cheap Set");
            testkit::set(&c, "SHD", "Pricey Set");
            testkit::simple(&c, "SOR", "1", "C");
            testkit::simple(&c, "SHD", "1", "P");
        }
        let mut conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        set_price(&shared, "SOR-1-normal", 5.00, "2026-06-01");
        set_price(&shared, "SHD-1-normal", 99.00, "2026-06-01");
        add_copy(&mut conn, "SOR-1-normal", "Near Mint", 1.00);
        add_copy(&mut conn, "SHD-1-normal", "Near Mint", 1.00);

        snapshot_today(&mut conn, "2026-06-02").unwrap();
        let series = value_history(&conn, "set").unwrap();
        assert_eq!(series.len(), 2);
        assert_eq!(
            series[0].bucket.as_deref(),
            Some("SHD"),
            "pricier set first"
        );
        assert_eq!(series[0].label.as_deref(), Some("Pricey Set"));
        assert_eq!(series[1].bucket.as_deref(), Some("SOR"));
    }

    fn target(handle: &str, path: std::path::PathBuf) -> FleetTarget {
        FleetTarget {
            handle: handle.into(),
            path,
        }
    }

    /// Acceptance, half one: two tenants each get their own value series, from
    /// their own copies, and neither's rows carry the other's holdings.
    #[test]
    fn two_tenants_each_get_a_value_series() {
        let (dir, mut alice, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.0, "2026-01-02");
        add_copy(&mut alice, "SOR-1-normal", "Near Mint", 1.0);
        drop(alice);
        let bob_path = dir.path().join("bob.sqlite");
        {
            let mut bob = connect_user(&bob_path, &shared).unwrap();
            add_copy(&mut bob, "SOR-1-normal", "Lightly Played", 1.0);
            add_copy(&mut bob, "SOR-1-normal", "Lightly Played", 1.0);
        }

        let targets = [
            target("alice", dir.path().join("collection.sqlite")),
            target("bob", bob_path.clone()),
        ];
        let report = snapshot_fleet(&shared, &targets, "2026-01-03");
        assert!(report.skipped.is_empty(), "{:?}", report.skipped);
        assert_eq!(report.valued.len(), 2);

        let a = connect_user(&targets[0].path, &shared).unwrap();
        let b = connect_user(&bob_path, &shared).unwrap();
        let av = value_history(&a, "all").unwrap();
        let bv = value_history(&b, "all").unwrap();
        assert_eq!(av[0].points.last().unwrap().market_value, 10.0);
        assert_eq!(av[0].points.last().unwrap().card_count, 1);
        // 2 copies at the Lightly Played multiplier 0.85.
        assert_eq!(bv[0].points.last().unwrap().market_value, 17.0);
        assert_eq!(bv[0].points.last().unwrap().card_count, 2);
    }

    /// Acceptance, half two: a tenant whose file cannot be read is skipped and
    /// named, and the others are still valued.
    #[test]
    fn an_unreadable_tenant_is_skipped_and_reported() {
        let (dir, mut alice, shared) = fixture();
        set_price(&shared, "SOR-1-normal", 10.0, "2026-01-02");
        add_copy(&mut alice, "SOR-1-normal", "Near Mint", 1.0);
        drop(alice);
        let bad = dir.path().join("bad.sqlite");
        std::fs::write(&bad, b"this is not a sqlite database, not even close.....").unwrap();
        let gone = dir.path().join("gone.sqlite");

        let report = snapshot_fleet(
            &shared,
            &[
                target("bad", bad),
                target("alice", dir.path().join("collection.sqlite")),
                target("gone", gone),
            ],
            "2026-01-03",
        );
        assert_eq!(report.valued.len(), 1);
        assert_eq!(report.valued[0].0, "alice");
        let skipped: Vec<&str> = report.skipped.iter().map(|(h, _)| h.as_str()).collect();
        assert_eq!(skipped, ["bad", "gone"]);
    }
}
