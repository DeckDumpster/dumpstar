//! The apply step: parse landed bytes and fold them into the catalog.
//!
//! Idempotent at two levels. A payload whose hash is already in
//! `applied_payloads` is skipped outright. A payload with a *new* hash but
//! identical content (the same catalog re-serialised) still writes nothing,
//! because every upsert is guarded by "only when a column differs" and prices
//! are insert-if-absent. The report counts rows actually written.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Transaction, params};

use crate::Catalog;
use crate::crosswalk::{self, OfficialPrinting};
use crate::error::{CatalogError, Result};
use crate::landing::Provenance;
use crate::record::{AlarmRecord, CatalogPayload, FORMAT};
use crate::swu::{self, CrawlMarker};
use crate::tcgcsv;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ApplyReport {
    /// The payload's hash was already applied; nothing was parsed.
    pub already_applied: bool,
    pub sets: usize,
    pub cards: usize,
    pub printings: usize,
    pub crosswalk: usize,
    pub prices: usize,
    /// Price rows addressed by a tcgcsv (productId, subTypeName) no printing
    /// carries. Counted, not dropped silently: the unmatched-printing alarm
    /// reads this.
    pub unmatched_prices: usize,
    pub formats: usize,
    pub format_rules: usize,
    pub legality: usize,
    pub groups: usize,
    pub products: usize,
    /// Alarm rows written or removed.
    pub alarms: usize,
    /// Official token backs and tcgcsv `X // Shield` products: handled, counted.
    pub token_backs: usize,
    /// Price rows for sealed products; they belong to the sealed-product series.
    pub sealed_prices: usize,
    /// Price rows for products a shared promo group lists for other sets.
    pub ignored_prices: usize,
    pub sync_state: usize,
}

impl ApplyReport {
    /// Whether the apply wrote no catalog row at all.
    pub fn is_noop(&self) -> bool {
        self.already_applied
            || (self.sets
                + self.cards
                + self.printings
                + self.crosswalk
                + self.prices
                + self.sealed_prices
                + self.formats
                + self.format_rules
                + self.legality
                + self.groups
                + self.products
                + self.alarms
                + self.sync_state
                == 0)
    }
}

/// `UPDATE ... WHERE` guard: true when any listed column differs from the
/// incoming row, NULL-safe.
fn differs(cols: &[&str]) -> String {
    cols.iter()
        .map(|c| format!("{c} IS NOT excluded.{c}"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

pub fn apply(catalog: &mut Catalog, bytes: &[u8], prov: &Provenance) -> Result<ApplyReport> {
    let tx = catalog.conn.transaction()?;
    let key = prov.applied_key();
    let seen = tx
        .prepare("SELECT 1 FROM applied_payloads WHERE payload_hash = ?1")?
        .exists([&key])?;
    if seen {
        return Ok(ApplyReport {
            already_applied: true,
            ..Default::default()
        });
    }
    let report = match prov.source.as_str() {
        swu::SRC_OFFICIAL => {
            let mut page = crate::official::parse_page(bytes)?;
            let mut known = BTreeMap::new();
            for c in page
                .payload
                .cards
                .iter()
                .filter(|c| c.card_id.starts_with(crate::official::PLACEHOLDER))
            {
                let hit: Option<String> = tx
                    .query_row(
                        "SELECT card_id FROM cards WHERE card_id NOT LIKE 'n:%' AND name = ?1 AND subtitle IS ?2 LIMIT 1",
                        params![c.name, c.subtitle],
                        |row| row.get(0),
                    )
                    .ok();
                if let Some(h) = hit {
                    known.insert(crate::official::card_name_key(c), h);
                }
            }
            crate::official::remap_cards(&mut page.payload, &known);
            let mut r = apply_payload(&tx, &page.payload)?;
            for p in &page.payload.printings {
                r.alarms += tx.execute(
                    "DELETE FROM alarms WHERE kind = 'official_bad_record' AND key = ?1",
                    [&p.printing_id],
                )?;
            }
            rejoin(&tx, &mut r)?;
            r
        }
        swu::SRC_OFFICIAL_CRAWL => {
            let m: CrawlMarker = serde_json::from_slice(bytes)?;
            let mut r = ApplyReport::default();
            if let Some(s) = &m.updated_since {
                r.sync_state += advance(&tx, swu::KEY_OFFICIAL_SINCE, s)?;
            }
            r.sync_state += advance(&tx, swu::KEY_OFFICIAL_CRAWLED_ON, &m.crawled_on)?;
            r
        }
        swu::SRC_GROUPS => {
            let mut r = ApplyReport::default();
            for g in tcgcsv::parse_groups(bytes)? {
                r.groups += tx.execute(
                    "INSERT INTO tcgcsv_groups (group_id, name, abbreviation) VALUES (?1,?2,?3)
                     ON CONFLICT(group_id) DO UPDATE SET name=excluded.name, abbreviation=excluded.abbreviation
                     WHERE name IS NOT excluded.name OR abbreviation IS NOT excluded.abbreviation",
                    params![g.group_id, g.name, g.abbreviation],
                )?;
                if let Some(d) = &g.published_on {
                    r.groups += tx.execute(
                        "INSERT INTO tcgcsv_group_dates (group_id, published_on) VALUES (?1,?2)
                         ON CONFLICT(group_id) DO UPDATE SET published_on=excluded.published_on
                         WHERE published_on IS NOT excluded.published_on",
                        params![g.group_id, d],
                    )?;
                }
            }
            rejoin(&tx, &mut r)?;
            r
        }
        swu::SRC_PRODUCTS => {
            let mut r = ApplyReport::default();
            for p in tcgcsv::parse_products(bytes)? {
                r.products += tx.execute(
                    "INSERT INTO tcgcsv_products (product_id, group_id, name, number) VALUES (?1,?2,?3,?4)
                     ON CONFLICT(product_id) DO UPDATE SET group_id=excluded.group_id, name=excluded.name, number=excluded.number
                     WHERE group_id IS NOT excluded.group_id OR name IS NOT excluded.name OR number IS NOT excluded.number",
                    params![p.product_id, p.group_id, p.name, p.number],
                )?;
            }
            for (id, url) in tcgcsv::parse_sealed_images(bytes)? {
                tx.execute(
                    "INSERT INTO sealed_images (product_id, source_url) VALUES (?1, ?2)
                     ON CONFLICT(product_id) DO UPDATE
                       SET source_url = excluded.source_url, retrieved_on = NULL, file = NULL
                     WHERE source_url IS NOT excluded.source_url",
                    params![id, url],
                )?;
            }
            rejoin(&tx, &mut r)?;
            r
        }
        swu::SRC_PRICES => {
            let day = prov.as_of.as_deref().ok_or_else(|| {
                CatalogError::Source("a tcgcsv price payload carries no day".into())
            })?;
            let mut payload = CatalogPayload::empty();
            payload.prices = tcgcsv::parse_prices(bytes, day)?;
            apply_payload(&tx, &payload)?
        }
        swu::SRC_LAST_UPDATED => {
            let stamp = String::from_utf8_lossy(bytes).trim().to_string();
            ApplyReport {
                sync_state: advance(&tx, swu::KEY_TCGCSV_LAST_UPDATED, &stamp)?,
                ..Default::default()
            }
        }
        _ => {
            let payload: CatalogPayload = serde_json::from_slice(bytes)?;
            if payload.format != FORMAT {
                return Err(CatalogError::Format(payload.format));
            }
            apply_payload(&tx, &payload)?
        }
    };
    tx.execute(
        "INSERT OR IGNORE INTO applied_payloads (payload_hash, source, url) VALUES (?1, ?2, ?3)",
        params![key, prov.source, prov.url],
    )?;
    tx.commit()?;
    Ok(report)
}

fn apply_payload(tx: &Transaction, p: &CatalogPayload) -> Result<ApplyReport> {
    let mut r = ApplyReport::default();

    let sql = format!(
        "INSERT INTO sets (set_code, name, release_date, sort_order) VALUES (?1,?2,?3,?4)
         ON CONFLICT(set_code) DO UPDATE SET name=excluded.name,
           release_date=COALESCE(excluded.release_date, release_date), sort_order=excluded.sort_order
         WHERE {}",
        "(name IS NOT excluded.name OR sort_order IS NOT excluded.sort_order \
          OR (excluded.release_date IS NOT NULL AND release_date IS NOT excluded.release_date))"
    );
    for s in &p.sets {
        r.sets += tx.execute(
            &sql,
            params![s.set_code, s.name, s.release_date, s.sort_order],
        )?;
    }

    let cols = [
        "name",
        "subtitle",
        "card_type",
        "cost",
        "power",
        "hp",
        "aspects",
        "traits",
        "arenas",
        "rules_text",
    ];
    let sql = format!(
        "INSERT INTO cards (card_id, name, subtitle, card_type, cost, power, hp, aspects, traits, arenas, rules_text)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
         ON CONFLICT(card_id) DO UPDATE SET {}
         WHERE {}",
        cols.iter().map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(", "),
        differs(&cols)
    );
    for c in &p.cards {
        let changed = tx.execute(
            &sql,
            params![
                c.card_id,
                c.name,
                c.subtitle,
                c.card_type,
                c.cost,
                c.power,
                c.hp,
                serde_json::to_string(&c.aspects)?,
                serde_json::to_string(&c.traits)?,
                serde_json::to_string(&c.arenas)?,
                c.rules_text,
            ],
        )?;
        if changed > 0 {
            tx.execute("DELETE FROM card_search WHERE card_id = ?1", [&c.card_id])?;
            tx.execute(
                "INSERT INTO card_search (card_id, name, subtitle, rules_text, traits)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    c.card_id,
                    c.name,
                    c.subtitle,
                    c.rules_text,
                    c.traits.join(" ")
                ],
            )?;
        }
        r.cards += changed;
    }

    let cols = [
        "card_id",
        "set_code",
        "number",
        "variant",
        "variant_code",
        "finish",
        "rarity",
        "artist",
        "image_url",
    ];
    let sql = format!(
        "INSERT INTO printings (printing_id, card_id, set_code, number, variant, variant_code, finish, rarity, artist, image_url)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
         ON CONFLICT(printing_id) DO UPDATE SET {}
         WHERE {}",
        cols.iter().map(|c| format!("{c}=excluded.{c}")).collect::<Vec<_>>().join(", "),
        differs(&cols)
    );
    let xw = "INSERT INTO printing_source_id (source, source_id, sub_type_name, printing_id)
              VALUES (?1,?2,?3,?4)
              ON CONFLICT(source, source_id, sub_type_name) DO UPDATE SET printing_id=excluded.printing_id
              WHERE printing_id IS NOT excluded.printing_id";
    for pr in &p.printings {
        r.printings += tx.execute(
            &sql,
            params![
                pr.printing_id,
                pr.card_id,
                pr.set_code,
                pr.number,
                pr.variant,
                pr.variant_code,
                pr.finish,
                pr.rarity,
                pr.artist,
                pr.image_url
            ],
        )?;
        if let Some(uid) = &pr.official_card_uid {
            r.crosswalk += tx.execute(xw, params!["official", uid, pr.finish, pr.printing_id])?;
        }
        if let (Some(pid), Some(sub)) = (pr.tcgcsv_product_id, &pr.tcgcsv_sub_type_name) {
            r.crosswalk +=
                tx.execute(xw, params!["tcgcsv", pid.to_string(), sub, pr.printing_id])?;
        }
    }

    for f in &p.formats {
        r.formats += tx.execute(
            "INSERT INTO formats (format_id, name) VALUES (?1,?2)
             ON CONFLICT(format_id) DO UPDATE SET name=excluded.name WHERE name IS NOT excluded.name",
            params![f.format_id, f.name],
        )?;
        for (k, v) in &f.rules {
            r.format_rules += tx.execute(
                "INSERT INTO format_rules (format_id, rule_key, rule_value) VALUES (?1,?2,?3)
                 ON CONFLICT(format_id, rule_key) DO UPDATE SET rule_value=excluded.rule_value
                 WHERE rule_value IS NOT excluded.rule_value",
                params![f.format_id, k, v],
            )?;
        }
    }
    for l in &p.legality {
        r.legality += tx.execute(
            "INSERT INTO card_legality (format_id, card_id, status) VALUES (?1,?2,?3)
             ON CONFLICT(format_id, card_id) DO UPDATE SET status=excluded.status
             WHERE status IS NOT excluded.status",
            params![l.format_id, l.card_id, l.status],
        )?;
    }

    // A product is linked under the sub type its prices arrive as, which the
    // join cannot know before the first day it is priced: join again once a
    // sub type is new, so that day's prices land on the link they belong to.
    let mut new_sub_type = false;
    for pr in &p.prices {
        new_sub_type |= tx.execute(
            "INSERT OR IGNORE INTO tcgcsv_sub_types (product_id, sub_type_name) VALUES (?1,?2)",
            params![pr.tcgcsv_product_id, pr.tcgcsv_sub_type_name],
        )? > 0;
    }
    if new_sub_type {
        rejoin(tx, &mut r)?;
    }
    for pr in &p.prices {
        let printing: Option<String> = tx
            .query_row(
                "SELECT printing_id FROM printing_source_id
                 WHERE source='tcgcsv' AND source_id=?1 AND sub_type_name=?2",
                params![pr.tcgcsv_product_id.to_string(), pr.tcgcsv_sub_type_name],
                |row| row.get(0),
            )
            .ok();
        let Some(printing) = printing else {
            // A product already alarmed (no official printing, ambiguous, an
            // unlisted promo) is reported once, as the product.
            let alarmed: bool = tx.query_row(
                "SELECT EXISTS (SELECT 1 FROM alarms WHERE key = ?1 AND kind IN
                   ('tcgcsv_without_official', 'ambiguous_tcgcsv', 'tcgcsv_unlisted_promo'))",
                [pr.tcgcsv_product_id.to_string()],
                |row| row.get(0),
            )?;
            if alarmed {
                r.unmatched_prices += 1;
                continue;
            }
            let product: Option<(Option<String>, i64, String)> = tx
                .query_row(
                    "SELECT number, group_id, name FROM tcgcsv_products WHERE product_id = ?1",
                    [pr.tcgcsv_product_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .ok();
            match product {
                Some((None, ..)) => {
                    r.sealed_prices += tx.execute(
                        "INSERT OR IGNORE INTO sealed_prices
                           (tcgplayer_product_id, low_price, mid_price, high_price,
                            market_price, direct_low_price, observed_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            pr.tcgcsv_product_id,
                            pr.low,
                            pr.mid,
                            pr.high,
                            pr.market,
                            pr.direct_low,
                            pr.observed_on
                        ],
                    )?
                }
                Some((Some(n), ..)) if crosswalk::is_token_back_number(&n) => r.token_backs += 1,
                Some((_, g, name))
                    if tcgcsv::group_info(g).is_some_and(|i| tcgcsv::is_ignored(&i, &name)) =>
                {
                    r.ignored_prices += 1
                }
                _ => {
                    r.unmatched_prices += 1;
                    r.alarms += put_alarms(
                        tx,
                        &[AlarmRecord::alarm(
                            "unmatched_price",
                            format!("{}:{}", pr.tcgcsv_product_id, pr.tcgcsv_sub_type_name),
                            format!(
                                "tcgcsv product {} ({}) priced on {} but joined to no printing",
                                pr.tcgcsv_product_id, pr.tcgcsv_sub_type_name, pr.observed_on
                            ),
                        )],
                        &[],
                    )?;
                }
            }
            continue;
        };
        r.prices += tx.execute(
            "INSERT OR IGNORE INTO prices (printing_id, source, observed_on, low, mid, high, market, direct_low)
             VALUES (?1,'tcgcsv',?2,?3,?4,?5,?6,?7)",
            params![printing, pr.observed_on, pr.low, pr.mid, pr.high, pr.market, pr.direct_low],
        )?;
    }
    r.alarms += put_alarms(tx, &p.alarms, &[])?;
    Ok(r)
}

/// Move a sync key forward; never backwards.
fn advance(tx: &Transaction, key: &str, value: &str) -> Result<usize> {
    Ok(tx.execute(
        "INSERT INTO sync_state (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value WHERE excluded.value > value",
        params![key, value],
    )?)
}

/// Upsert `alarms`; then delete every stored alarm of a kind in `replace` that
/// is not among them. Returns rows written or removed.
fn put_alarms(tx: &Transaction, alarms: &[AlarmRecord], replace: &[&str]) -> Result<usize> {
    let mut n = 0;
    for a in alarms {
        n += tx.execute(
            "INSERT INTO alarms (kind, key, severity, detail) VALUES (?1,?2,?3,?4)
             ON CONFLICT(kind, key) DO UPDATE SET severity=excluded.severity, detail=excluded.detail
             WHERE severity IS NOT excluded.severity OR detail IS NOT excluded.detail",
            params![a.kind, a.key, a.severity, a.detail],
        )?;
    }
    let keep: BTreeSet<(&str, &str)> = alarms
        .iter()
        .map(|a| (a.kind.as_str(), a.key.as_str()))
        .collect();
    for kind in replace {
        let existing: Vec<String> = tx
            .prepare("SELECT key FROM alarms WHERE kind = ?1")?
            .query_map([kind], |row| row.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        for key in existing {
            if !keep.contains(&(*kind, key.as_str())) {
                n += tx.execute(
                    "DELETE FROM alarms WHERE kind = ?1 AND key = ?2",
                    params![kind, key],
                )?;
            }
        }
    }
    Ok(n)
}

/// Dates for sets no source carries (set_code, release_date, source), merged
/// after the source dates and only into sets still undated.
const REMEDIAL_RELEASE_DATES: &str = include_str!("../data/remedial/set-release-dates.csv");

const DATE_KINDS: &[&str] = &["set_without_release_date"];

/// Write each set's release date from the publish date of its main tcgcsv group
/// (the official API carries none), and raise a named alarm for every set that
/// still has no date, so it is never left silently NULL.
fn stamp_release_dates(tx: &Transaction, r: &mut ApplyReport) -> Result<()> {
    let set_codes: BTreeSet<String> = tx
        .prepare("SELECT set_code FROM sets")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let rows: Vec<(i64, Option<String>, String)> = tx
        .prepare(
            "SELECT g.group_id, g.abbreviation, d.published_on
             FROM tcgcsv_groups g JOIN tcgcsv_group_dates d USING (group_id) ORDER BY g.group_id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let mut dates: BTreeMap<String, String> = BTreeMap::new();
    for (_, abbr, date) in &rows {
        if let Some(a) = abbr
            && set_codes.contains(a)
        {
            dates.insert(a.clone(), date.clone());
        }
    }
    for row in tcgcsv::group_table() {
        if row.info.kind == tcgcsv::GroupKind::Main
            && let Some((_, _, date)) = rows.iter().find(|x| x.0 == row.group_id)
        {
            dates.insert(row.info.set_code, date.clone());
        }
    }
    for (code, date) in &dates {
        r.sets += tx.execute(
            "UPDATE sets SET release_date = ?2 WHERE set_code = ?1 AND release_date IS NOT ?2",
            params![code, date],
        )?;
    }
    for line in REMEDIAL_RELEASE_DATES.lines().skip(1) {
        let mut f = line.splitn(3, ',');
        if let (Some(code), Some(date)) = (f.next(), f.next()) {
            r.sets += tx.execute(
                "UPDATE sets SET release_date = ?2 WHERE set_code = ?1 AND release_date IS NULL",
                params![code.trim(), date.trim()],
            )?;
        }
    }
    let missing: Vec<(String, String)> = tx
        .prepare("SELECT set_code, name FROM sets WHERE release_date IS NULL ORDER BY set_code")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let alarms: Vec<AlarmRecord> = missing
        .into_iter()
        .map(|(code, name)| {
            AlarmRecord::alarm(
                "set_without_release_date",
                code,
                format!("set {name:?} has no release date in any source"),
            )
        })
        .collect();
    r.alarms += put_alarms(tx, &alarms, DATE_KINDS)?;
    Ok(())
}

const JOIN_KINDS: &[&str] = &[
    "official_without_tcgcsv",
    "official_unlisted",
    "tcgcsv_without_official",
    "ambiguous_tcgcsv",
    "unmapped_group",
    "tcgcsv_unlisted_promo",
    "base_listed_per_token",
    "remedial_stale",
    "remedial_conflict",
];

/// Re-run the official <-> tcgcsv join over everything stored, so that a change
/// on either side re-joins the other. A catalog with no tcgcsv products stored
/// (the fake, or before the first price sync) is left exactly as it is.
fn rejoin(tx: &Transaction, r: &mut ApplyReport) -> Result<()> {
    stamp_release_dates(tx, r)?;
    let products: Vec<tcgcsv::ProductRecord> = tx
        .prepare(
            "SELECT product_id, group_id, name, number FROM tcgcsv_products ORDER BY product_id",
        )?
        .query_map([], |row| {
            Ok(tcgcsv::ProductRecord {
                product_id: row.get(0)?,
                group_id: row.get(1)?,
                name: row.get(2)?,
                number: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    if products.is_empty() {
        return Ok(());
    }
    let set_codes: BTreeSet<String> = tx
        .prepare("SELECT set_code FROM sets")?
        .query_map([], |row| row.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let rows: Vec<(i64, String, Option<String>)> = tx
        .prepare("SELECT group_id, name, abbreviation FROM tcgcsv_groups")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    let group_names: BTreeMap<i64, String> = rows.iter().map(|(i, n, _)| (*i, n.clone())).collect();
    let groups = tcgcsv::group_infos(&rows, &set_codes);
    let printings: Vec<OfficialPrinting> = tx
        .prepare(
            "SELECT p.printing_id, p.set_code, p.number, p.variant, p.variant_code, p.finish, c.name, c.subtitle
             FROM printings p JOIN cards c ON c.card_id = p.card_id ORDER BY p.printing_id",
        )?
        .query_map([], |row| {
            Ok(OfficialPrinting {
                printing_id: row.get(0)?,
                set_code: row.get(1)?,
                number: row.get(2)?,
                variant: row.get(3)?,
                variant_code: row.get(4)?,
                finish: row.get(5)?,
                name: row.get(6)?,
                subtitle: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<_, _>>()?;
    let mut extra = crosswalk::Extra {
        remedial: &crosswalk::remedial_table(),
        ..Default::default()
    };
    for (product, sub_type) in tx
        .prepare("SELECT product_id, sub_type_name FROM tcgcsv_sub_types")?
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?
    {
        extra.sub_types.entry(product).or_default().push(sub_type);
    }
    let joined = crosswalk::join_with(&printings, &products, &groups, &group_names, &extra);
    r.token_backs = joined.token_backs;

    let want: BTreeSet<(String, String, String)> = joined
        .links
        .iter()
        .map(|l| {
            (
                l.product_id.to_string(),
                l.sub_type_name.clone(),
                l.printing_id.clone(),
            )
        })
        .collect();
    let have: Vec<(String, String, String)> = tx
        .prepare("SELECT source_id, sub_type_name, printing_id FROM printing_source_id WHERE source = 'tcgcsv'")?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<std::result::Result<_, _>>()?;
    for row in have.iter().filter(|h| !want.contains(*h)) {
        r.crosswalk += tx.execute(
            "DELETE FROM printing_source_id WHERE source='tcgcsv' AND source_id=?1 AND sub_type_name=?2 AND printing_id=?3",
            params![row.0, row.1, row.2],
        )?;
    }
    for l in &joined.links {
        r.crosswalk += tx.execute(
            "INSERT INTO printing_source_id (source, source_id, sub_type_name, printing_id)
             VALUES ('tcgcsv', ?1, ?2, ?3)
             ON CONFLICT(source, source_id, sub_type_name) DO UPDATE SET printing_id=excluded.printing_id
             WHERE printing_id IS NOT excluded.printing_id",
            params![l.product_id.to_string(), l.sub_type_name, l.printing_id],
        )?;
    }
    r.alarms += put_alarms(tx, &joined.alarms, JOIN_KINDS)?;
    r.alarms += tx.execute(
        "DELETE FROM alarms WHERE kind = 'unmatched_price' AND EXISTS (
           SELECT 1 FROM printing_source_id s WHERE s.source = 'tcgcsv'
             AND s.source_id || ':' || s.sub_type_name = alarms.key)",
        [],
    )?;
    Ok(())
}
