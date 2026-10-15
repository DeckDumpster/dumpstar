//! tcgcsv (category 79, Star Wars: Unlimited): group, product and price
//! responses parsed into records. tcgcsv supplies only prices and productIds;
//! it never writes an identity, text or art field.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CatalogError, Result};
use crate::record::PriceRecord;

pub const CATEGORY: i64 = 79;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GroupRecord {
    pub group_id: i64,
    pub name: String,
    pub abbreviation: Option<String>,
    /// `publishedOn` cut to its date (`YYYY-MM-DD`); None when absent or unreadable.
    pub published_on: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProductRecord {
    pub product_id: i64,
    pub group_id: i64,
    pub name: String,
    /// `extendedData.Number`; None for a sealed product.
    pub number: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// Number is the set's collector number; a `(Hyperspace)`/`(Showcase)` name
    /// suffix picks the variant.
    Main,
    /// Number is `N/total` within the promo group; a `(Hyperspace)` suffix is the
    /// Hyperspace printing of official number N, anything else a Weekly Play.
    WeeklyPlay,
    /// A promo group shared by many sets: only its `(Hyperspace)` products are
    /// read, by number; everything else in it is not this set's.
    HyperspaceOnly,
    /// A prize/event group whose numbers mean nothing to the official catalog:
    /// products are matched on name, with the parenthetical (`Top 8`) naming the
    /// finish. `variants` are the official variants the group lists.
    Promo,
    /// A group of one official set whose numbers mean nothing to the official
    /// catalog (convention exclusives, season promos): products are matched on
    /// name and parenthetical, within that set only.
    Listed,
}

#[derive(Debug, Clone)]
pub struct GroupInfo {
    pub set_code: String,
    pub kind: GroupKind,
    pub variants: Vec<String>,
}

pub struct GroupRow {
    pub group_id: i64,
    pub info: GroupInfo,
}

/// The hand-kept table (`groups.tsv`).
pub fn group_table() -> Vec<GroupRow> {
    include_str!("groups.tsv")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let group_id = f.next()?.trim().parse().ok()?;
            let set_code = f.next()?.trim().to_string();
            let kind = match f.next()?.trim() {
                "weekly_play" => GroupKind::WeeklyPlay,
                "hyperspace_only" => GroupKind::HyperspaceOnly,
                "promo" => GroupKind::Promo,
                "listed" => GroupKind::Listed,
                _ => GroupKind::Main,
            };
            let variants = f
                .next()
                .map(|v| {
                    v.split('|')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            Some(GroupRow {
                group_id,
                info: GroupInfo {
                    set_code,
                    kind,
                    variants,
                },
            })
        })
        .collect()
}

/// The set a Weekly Play Promos group (`SEC-WPP`, `ASHWPP`) belongs to, given the
/// official set codes: `<set>P` when the official catalog files the promos in a
/// set of their own, else a [`GroupKind::WeeklyPlay`] group of `<set>`.
pub fn weekly_play_group(abbreviation: &str, is_set: &dyn Fn(&str) -> bool) -> Option<GroupInfo> {
    let base = abbreviation.strip_suffix("WPP")?.trim_end_matches('-');
    let own = format!("{base}P");
    if is_set(&own) {
        return Some(GroupInfo {
            set_code: own,
            kind: GroupKind::Main,
            variants: vec![],
        });
    }
    is_set(base).then(|| GroupInfo {
        set_code: base.to_string(),
        kind: GroupKind::WeeklyPlay,
        variants: vec![],
    })
}

/// Every group the join reads: the hand-kept table over groups whose
/// abbreviation names an official set, and the Weekly Play Promos groups.
pub fn group_infos(
    rows: &[(i64, String, Option<String>)],
    set_codes: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeMap<i64, GroupInfo> {
    let is_set = |s: &str| set_codes.contains(s);
    let mut groups = std::collections::BTreeMap::new();
    for (id, _, abbr) in rows {
        let Some(a) = abbr else { continue };
        if let Some(info) = weekly_play_group(a, &is_set) {
            groups.insert(*id, info);
        } else if set_codes.contains(a) {
            groups.insert(
                *id,
                GroupInfo {
                    set_code: a.clone(),
                    kind: GroupKind::Main,
                    variants: vec![],
                },
            );
        }
    }
    for row in group_table() {
        groups.insert(row.group_id, row.info);
    }
    groups
}

/// The hand-kept row for one group, if there is one.
pub fn group_info(group_id: i64) -> Option<GroupInfo> {
    group_table()
        .into_iter()
        .find(|g| g.group_id == group_id)
        .map(|g| g.info)
}

/// Whether a product in a hyperspace-only group is outside this catalog.
pub fn is_ignored(info: &GroupInfo, name: &str) -> bool {
    info.kind == GroupKind::HyperspaceOnly && !name.trim_end().ends_with("(Hyperspace)")
}

fn results(bytes: &[u8]) -> Result<Vec<Value>> {
    let v: Value = serde_json::from_slice(bytes)?;
    v.get("results")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| CatalogError::Source("tcgcsv response has no results".into()))
}

pub fn parse_groups(bytes: &[u8]) -> Result<Vec<GroupRecord>> {
    Ok(results(bytes)?
        .iter()
        .filter_map(|g| {
            Some(GroupRecord {
                group_id: g.get("groupId")?.as_i64()?,
                name: g.get("name")?.as_str()?.to_string(),
                abbreviation: g
                    .get("abbreviation")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
                published_on: g
                    .get("publishedOn")
                    .and_then(Value::as_str)
                    .and_then(|d| d.get(..10))
                    .filter(|d| {
                        d.len() == 10
                            && d.bytes().enumerate().all(|(i, b)| {
                                if i == 4 || i == 7 {
                                    b == b'-'
                                } else {
                                    b.is_ascii_digit()
                                }
                            })
                    })
                    .map(str::to_string),
            })
        })
        .collect())
}

pub fn parse_products(bytes: &[u8]) -> Result<Vec<ProductRecord>> {
    results(bytes)?
        .iter()
        .map(|p| {
            let bad = || CatalogError::Source(format!("tcgcsv product unreadable: {p}"));
            Ok(ProductRecord {
                product_id: p.get("productId").and_then(Value::as_i64).ok_or_else(bad)?,
                group_id: p.get("groupId").and_then(Value::as_i64).ok_or_else(bad)?,
                name: p
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .into(),
                number: p
                    .get("extendedData")
                    .and_then(Value::as_array)
                    .and_then(|e| {
                        e.iter()
                            .find(|d| d.get("name").and_then(Value::as_str) == Some("Number"))
                    })
                    .and_then(|d| d.get("value")?.as_str())
                    .map(|s| s.trim().to_string()),
            })
        })
        .collect()
}

pub fn parse_prices(bytes: &[u8], observed_on: &str) -> Result<Vec<PriceRecord>> {
    results(bytes)?
        .iter()
        .map(|r| {
            let bad = || CatalogError::Source(format!("tcgcsv price unreadable: {r}"));
            let f = |k: &str| r.get(k).and_then(Value::as_f64);
            Ok(PriceRecord {
                tcgcsv_product_id: r.get("productId").and_then(Value::as_i64).ok_or_else(bad)?,
                tcgcsv_sub_type_name: r
                    .get("subTypeName")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .into(),
                observed_on: observed_on.into(),
                low: f("lowPrice"),
                mid: f("midPrice"),
                high: f("highPrice"),
                market: f("marketPrice"),
                direct_low: f("directLowPrice"),
            })
        })
        .collect()
}

/// `last-updated.txt` is an ISO timestamp, e.g. `2026-10-12T20:05:44+0000`;
/// the day it names is the day its prices describe.
pub fn stamp_day(stamp: &str) -> Option<&str> {
    let d = stamp.trim().get(..10)?;
    let ok = d.bytes().enumerate().all(|(i, b)| {
        if i == 4 || i == 7 {
            b == b'-'
        } else {
            b.is_ascii_digit()
        }
    });
    ok.then_some(d)
}
