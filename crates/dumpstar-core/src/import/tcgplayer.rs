//! TCGplayer mass-entry / order-history CSV import (PLAN.md §9.2) — the
//! runner-up format, mostly used to back-fill order history.
//!
//! TCGplayer collection exports carry columns along the lines of:
//! `Quantity, Name, Simple Name, Set, Card Number, Set Code, Printing,
//! Condition, Language, Rarity, Product ID, SKU, Price`. Columns are
//! resolved by header name, so order and extras do not matter.

use std::collections::HashMap;

use super::{ImportError, ParsedRow, Result, map_condition, map_language};

/// Parse a TCGplayer collection / order CSV into one `ParsedRow` per copy.
pub fn parse(input: &str) -> Result<Vec<ParsedRow>> {
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
    // First header in `names` that is present.
    let first = |names: &[&str]| names.iter().find_map(|n| headers.get(*n).copied());

    let col_number = first(&["card number", "collector number", "number"])
        .ok_or_else(|| ImportError::MissingColumn("Card Number".into()))?;
    let col_set_code = first(&["set code"]);
    let col_set_name = first(&["set", "set name"]);
    if col_set_code.is_none() && col_set_name.is_none() {
        return Err(ImportError::MissingColumn("Set / Set Code".into()));
    }
    let col_printing = first(&["printing"]);
    let col_quantity = first(&["quantity", "qty", "add to quantity"]);
    let col_condition = first(&["condition"]);
    let col_language = first(&["language"]);
    let col_price = first(&["purchase price", "price"]);

    let mut rows = Vec::new();
    for (rec_i, record) in reader.records().enumerate() {
        let record = record?;
        let line = rec_i + 2; // 1-based; +1 for the consumed header row.
        let get = |c: Option<usize>| c.and_then(|i| record.get(i)).unwrap_or("").trim();

        let number = get(Some(col_number)).to_string();
        if number.is_empty() {
            return Err(ImportError::BadRow {
                line,
                message: "empty Card Number".into(),
            });
        }

        let set_code = get(col_set_code);
        let set_name = get(col_set_name);
        let set_hint = if set_code.is_empty() {
            set_name
        } else {
            set_code
        };
        if set_hint.is_empty() {
            return Err(ImportError::BadRow {
                line,
                message: "empty Set".into(),
            });
        }

        let quantity: u32 = {
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

        let purchase_price = {
            // TCGplayer prices may carry a leading '$'.
            let raw = get(col_price).trim_start_matches('$');
            if raw.is_empty() {
                None
            } else {
                Some(raw.parse().map_err(|_| ImportError::BadRow {
                    line,
                    message: format!("non-numeric Price '{raw}'"),
                })?)
            }
        };

        let variant = map_variant(get(col_printing));
        let condition = map_condition(get(col_condition));
        let language = map_language(get(col_language));

        for _ in 0..quantity {
            rows.push(ParsedRow {
                source_line: line,
                set_hint: set_hint.to_string(),
                set_name: (!set_name.is_empty()).then(|| set_name.to_string()),
                name: None,
                number: number.clone(),
                variant: variant.clone(),
                condition: condition.clone(),
                language: language.clone(),
                purchase_price,
                acquired_at: None,
                tags: Vec::new(),
            });
        }
    }
    Ok(rows)
}

/// TCGplayer `Printing` value → Dumpstar variant code.
fn map_variant(printing: &str) -> String {
    match printing.trim().to_lowercase().as_str() {
        // Star Wars: Unlimited prints in two finishes, Normal and Foil. The foil
        // finish is stored under the catalog's `holo` variant code.
        "foil" => "holo",
        // "normal", "" and anything unrecognised.
        _ => "normal",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
Quantity,Name,Set,Card Number,Set Code,Printing,Condition,Language,Price
3,Luke Skywalker - Faithful Friend,Spark of Rebellion,5,SOR,Foil,Near Mint,English,$12.50
1,Millennium Falcon - Piece of Junk,Spark of Rebellion,193,SOR,Normal,Lightly Played Foil,English,";

    #[test]
    fn parses_printing_price_and_quantity() {
        let rows = parse(SAMPLE).unwrap();
        assert_eq!(rows.len(), 4); // 3 + 1

        let luke = &rows[0];
        assert_eq!(luke.set_hint, "SOR");
        assert_eq!(luke.set_name.as_deref(), Some("Spark of Rebellion"));
        assert_eq!(luke.number, "5");
        assert_eq!(luke.variant, "holo");
        assert_eq!(luke.condition, "Near Mint");
        assert_eq!(luke.purchase_price, Some(12.50));

        let falcon = &rows[3];
        assert_eq!(falcon.variant, "normal");
        assert_eq!(falcon.condition, "Lightly Played"); // ' Foil' suffix stripped
        assert_eq!(falcon.purchase_price, None);
    }

    #[test]
    fn missing_card_number_column_is_an_error() {
        let csv = "Quantity,Set\n1,Spark of Rebellion";
        assert!(matches!(parse(csv), Err(ImportError::MissingColumn(_))));
    }
}
