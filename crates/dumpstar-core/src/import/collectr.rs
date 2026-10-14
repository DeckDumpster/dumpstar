//! Collectr CSV import (PLAN.md §9). Collectr (getcollectr.com) exports a
//! single file mixing single cards and sealed products across multiple
//! game categories. This parser splits that file along the garden wall:
//! singles become [`ParsedRow`]s, sealed products become
//! [`ParsedSealedRow`]s, and everything else (rows from other games) is reported
//! as skipped rather than silently dropped.
//!
//! Collectr columns: `Portfolio Name, Category, Set, Product Name,
//! Card Number, Rarity, Variance, Grade, Card Condition, Average Cost Paid,
//! Quantity, Market Price (As of <date>), Price Override, Watchlist,
//! Date Added, Notes`.
//!
//! The single/sealed discriminator is the **Card Number**: sealed products
//! carry none. Market Price is the platform's own valuation and is ignored —
//! Dumpstar prices from its own catalog.

use std::collections::HashMap;

use super::{
    ImportError, ParsedRow, ParsedSealedRow, Result, manabox, map_condition, map_language,
};

/// A row skipped during a Collectr import, with a reason — surfaced in the
/// preview so nothing disappears without explanation.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SkippedRow {
    pub source_line: usize,
    pub category: String,
    pub name: String,
    pub reason: String,
}

/// The split result of parsing a Collectr export.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct CollectrParsed {
    pub singles: Vec<ParsedRow>,
    pub sealed: Vec<ParsedSealedRow>,
    pub skipped: Vec<SkippedRow>,
}

/// Parse a Collectr collection export, routing each row to singles, sealed,
/// or skipped.
pub fn parse(input: &str) -> Result<CollectrParsed> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(input.as_bytes());

    let headers: HashMap<String, usize> = reader
        .headers()?
        .iter()
        .enumerate()
        .map(|(i, h)| (h.trim().to_lowercase(), i))
        .collect();
    // The market-price header carries a date suffix, so match by prefix.
    let idx = |name: &str| headers.get(name).copied();

    let col_category =
        idx("category").ok_or_else(|| ImportError::MissingColumn("Category".into()))?;
    let col_set = idx("set").ok_or_else(|| ImportError::MissingColumn("Set".into()))?;
    let col_name =
        idx("product name").ok_or_else(|| ImportError::MissingColumn("Product Name".into()))?;
    let col_number =
        idx("card number").ok_or_else(|| ImportError::MissingColumn("Card Number".into()))?;
    let col_variance = idx("variance");
    let col_condition = idx("card condition");
    let col_cost = idx("average cost paid");
    let col_quantity = idx("quantity");
    let col_date = idx("date added");
    let col_notes = idx("notes");

    let mut out = CollectrParsed {
        singles: Vec::new(),
        sealed: Vec::new(),
        skipped: Vec::new(),
    };

    for (rec_i, record) in reader.records().enumerate() {
        let record = record?;
        let line = rec_i + 2; // 1-based; +1 for the consumed header row.
        let get = |c: Option<usize>| c.and_then(|i| record.get(i)).unwrap_or("").trim();

        let category = get(Some(col_category)).to_string();
        let name = get(Some(col_name)).to_string();
        let set = get(Some(col_set));

        // Dumpstar tracks Star Wars: Unlimited — drop other games, but say so.
        if !is_swu_category(&category) {
            out.skipped.push(SkippedRow {
                source_line: line,
                category: category.clone(),
                name,
                reason: format!("not a Star Wars: Unlimited category '{category}'"),
            });
            continue;
        }
        if set.is_empty() {
            out.skipped.push(SkippedRow {
                source_line: line,
                category,
                name,
                reason: "empty Set".into(),
            });
            continue;
        }

        let quantity = {
            let raw = get(col_quantity);
            if raw.is_empty() {
                1
            } else {
                raw.parse().map_err(|_| ImportError::BadRow {
                    line,
                    message: format!("non-numeric Quantity '{raw}'"),
                })?
            }
        };
        let condition = map_condition(get(col_condition));
        let purchase_price = parse_price(get(col_cost));
        let date_added = get(col_date);

        // The garden wall: a blank Card Number means a sealed product.
        let number_raw = get(Some(col_number));
        if number_raw.is_empty() {
            out.sealed.push(ParsedSealedRow {
                source_line: line,
                name,
                set_hint: set.to_string(),
                set_name: Some(set.to_string()),
                category_hint: None,
                quantity,
                condition,
                purchase_price,
                purchase_date: (!date_added.is_empty()).then(|| date_added.to_string()),
                notes: clean_notes(get(col_notes)),
            });
            continue;
        }

        let variant = manabox::map_variant(get(col_variance));
        let acquired_at = (!date_added.is_empty()).then(|| to_rfc3339(date_added));
        let single = ParsedRow {
            source_line: line,
            set_hint: set.to_string(),
            set_name: Some(set.to_string()),
            name: (!name.is_empty()).then(|| name.clone()),
            number: normalize_number(number_raw),
            variant,
            condition,
            language: map_language(""), // Collectr carries no language column.
            purchase_price,
            acquired_at,
            tags: Vec::new(),
        };
        // Singles are strictly one row per physical card.
        for _ in 0..quantity {
            out.singles.push(single.clone());
        }
    }
    Ok(out)
}

/// Collectr writes collector numbers as `number/total` (`221/217`),
/// occasionally with a set-abbrev prefix (`P 176`), and zero-padded to
/// three digits (`090/086`). Reduce to the bare, unpadded collector number
/// the catalog stores.
fn normalize_number(raw: &str) -> String {
    let s = raw.trim();
    let s = s.split('/').next().unwrap_or(s).trim();
    // "P 176" → "176": drop an alphabetic prefix before a numeric tail.
    let s = if let Some((prefix, tail)) = s.rsplit_once(' ')
        && !tail.is_empty()
        && tail.chars().all(|c| c.is_ascii_digit())
        && prefix.chars().any(|c| c.is_ascii_alphabetic())
    {
        tail
    } else {
        s
    };
    // "090" → "90": strip zero-padding, but only on all-digit tokens.
    super::normalize_collector_number(s)
}

/// Parse a Collectr price cell. Empty → `None`; a literal `0` is kept as
/// `Some(0.0)` (a recorded zero cost). Tolerant of junk → `None` so one bad
/// cell never fails the whole import.
fn parse_price(raw: &str) -> Option<f64> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    s.parse().ok()
}

/// A bare `YYYY-MM-DD` date → an RFC-3339 midnight-UTC timestamp, so it sorts
/// alongside the timestamps the rest of the app writes. Non-date input is
/// passed through unchanged.
fn to_rfc3339(date: &str) -> String {
    let d = date.trim();
    if d.len() == 10 && d.as_bytes()[4] == b'-' && d.as_bytes()[7] == b'-' {
        format!("{d}T00:00:00Z")
    } else {
        d.to_string()
    }
}

/// Collectr sometimes leaves stray `; ` separators in the Notes cell. Trim
/// them; an all-separator cell becomes `None`.
fn clean_notes(raw: &str) -> Option<String> {
    let s = raw
        .trim()
        .trim_matches(|c: char| c == ';' || c.is_whitespace());
    (!s.is_empty()).then(|| s.to_string())
}

/// Collectr's category cell for Star Wars: Unlimited, tolerant of punctuation
/// and case ("Star Wars Unlimited", "Star Wars: Unlimited").
fn is_swu_category(category: &str) -> bool {
    let key: String = category
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    key == "starwarsunlimited"
}

#[cfg(test)]
mod tests {
    use super::*;

    // A representative slice of a real Collectr export: a Lorcana row (must
    // be skipped), singles with slashed/prefixed numbers and both variances,
    // and sealed products with quantities.
    const SAMPLE: &str = "\
Portfolio Name,Category,Set,Product Name,Card Number,Rarity,Variance,Grade,Card Condition,Average Cost Paid,Quantity,Market Price (As of 2026-07-17),Price Override,Watchlist,Date Added,Notes
Main,Lorcana,Disney Lorcana Promo Cards,Elsa - The Fifth Spirit,6,Promo,Holofoil,Ungraded,Near Mint,0.0000,1,7.12,0,false,2026-01-24,
Main,Star Wars Unlimited,Spark of Rebellion,Luke Skywalker - Jedi Knight,005/252,Legendary,Foil,Ungraded,Near Mint,0,1,7.19,0,false,2026-04-14,
Main,Star Wars Unlimited,Prerelease Promos,Darth Vader - Dark Lord of the Sith,P 176,Promo,Foil,Ungraded,Near Mint,0,2,41.43,0,false,2026-03-30,
Main,Star Wars Unlimited,Spark of Rebellion,Boba Fett - Disintegrator,026,Rare,Normal,Ungraded,Near Mint,0,1,14.69,0,false,2026-04-02,
Sealed Star Wars Unlimited,Star Wars Unlimited,Spark of Rebellion,Spark of Rebellion Two-Player Starter,,,Normal,Ungraded,Near Mint,75.0000,1,135.35,0,false,2026-02-28,
Sealed Star Wars Unlimited,Star Wars Unlimited,Twilight of the Republic,Twilight of the Republic Booster Display,,,Normal,Ungraded,Near Mint,200.0000,2,200.26,0,false,2026-03-23,; ";

    #[test]
    fn splits_singles_sealed_and_skips_other_games() {
        let out = parse(SAMPLE).unwrap();

        // Lorcana row is skipped, not imported.
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].category, "Lorcana");

        // Singles: Luke(x1) + Vader(x2, expanded) + Boba(x1) = 4 copies.
        assert_eq!(out.singles.len(), 4);
        // Sealed: 2 distinct products (quantities kept, not expanded).
        assert_eq!(out.sealed.len(), 2);
    }

    #[test]
    fn normalizes_numbers_and_variants() {
        let out = parse(SAMPLE).unwrap();
        let luke = &out.singles[0];
        assert_eq!(luke.number, "5"); // 005/252 → 5
        assert_eq!(luke.variant, "holo"); // Foil → holo
        assert_eq!(luke.set_hint, "Spark of Rebellion");
        assert_eq!(luke.acquired_at.as_deref(), Some("2026-04-14T00:00:00Z"));

        let vader = &out.singles[1];
        assert_eq!(vader.number, "176"); // P 176 → 176
        assert_eq!(out.singles[1], out.singles[2]); // both expanded copies identical

        let boba = &out.singles[3];
        assert_eq!(boba.number, "26"); // 026 → 26 (zero-padding stripped)
        assert_eq!(boba.variant, "normal"); // Variance Normal → normal
    }

    #[test]
    fn strips_zero_padding_but_not_alphanumeric() {
        // Collectr zero-pads to three digits; the catalog stores unpadded.
        assert_eq!(normalize_number("090/086"), "90");
        assert_eq!(normalize_number("009"), "9");
        assert_eq!(normalize_number("P 176"), "176");
        // Alphanumeric collector numbers must survive untouched.
        assert_eq!(normalize_number("X01"), "X01");
        assert_eq!(normalize_number("ABC123"), "ABC123");
        assert_eq!(normalize_number("X01/X30"), "X01");
    }

    #[test]
    fn sealed_keeps_quantity_and_cleans_notes() {
        let out = parse(SAMPLE).unwrap();
        let starter = &out.sealed[0];
        assert_eq!(starter.name, "Spark of Rebellion Two-Player Starter");
        assert_eq!(starter.quantity, 1);
        assert_eq!(starter.purchase_price, Some(75.0));
        assert_eq!(starter.purchase_date.as_deref(), Some("2026-02-28"));

        let box_ = &out.sealed[1];
        assert_eq!(box_.quantity, 2); // not expanded
        assert_eq!(box_.notes, None); // stray "; " stripped away
    }
}
