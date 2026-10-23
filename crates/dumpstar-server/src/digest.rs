//! The `lines_digest` of the `/mcp` `get_deck` result, part of that tool's contract.
//!
//! `get_deck` returns `{deck, lines, lines_digest, cards}`. `lines_digest` is the SHA-256
//! (lowercase hex) of the lines and their card objects in canonical form, so a client that copies
//! a deck document can prove it is the saved deck, unaltered: [`verify_digest`] recomputes it.

use serde_json::Value;

/// The card fields the digest covers, per line. Each is a JSON pointer into the card object;
/// arrays are sorted. Changing this list changes every digest, so it is part of the contract.
pub const DIGEST_CARD_FIELDS: [&str; 9] = [
    "/name",
    "/subtitle",
    "/type",
    "/rarity",
    "/cost",
    "/game/hp",
    "/game/epic_action",
    "/game/aspects",
    "/game/arenas",
];

/// The digest `get_deck` returns as `lines_digest`: SHA-256 (lowercase hex) over the canonical form
/// of the deck's lines: per line one `LF`-terminated row of `TAB`-separated cells, sorted as
/// rows: `card_id`, `qty`, `reserved`, then the line's card object's [`DIGEST_CARD_FIELDS`], each
/// as compact JSON (`null` when absent; arrays sorted). `zone` is not part of it. `None` when
/// `lines` is not an array of lines with a card id and a qty.
pub fn lines_digest(lines: &Value, cards: &Value) -> Option<String> {
    use sha2::{Digest, Sha256};
    let mut rows = Vec::new();
    for l in lines.as_array()? {
        let id = l["card_id"].as_str()?;
        let mut row = vec![
            id.to_string(),
            l["qty"].as_u64()?.to_string(),
            l["reserved"].as_u64().unwrap_or(0).to_string(),
        ];
        let card = cards.get(id).unwrap_or(&Value::Null);
        for ptr in DIGEST_CARD_FIELDS {
            let mut v = card.pointer(ptr).cloned().unwrap_or(Value::Null);
            if let Value::Array(a) = &mut v {
                a.sort_by_key(|x| x.to_string());
            }
            row.push(v.to_string());
        }
        rows.push(row.join("\t"));
    }
    rows.sort();
    let canon: String = rows.iter().map(|r| format!("{r}\n")).collect();
    Some(
        Sha256::digest(canon.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
    )
}

/// Refuses a document whose `lines_digest` does not match its own `lines` and `cards`: it is not
/// the saved deck `get_deck` returned. A document with no `lines_digest` passes.
#[allow(dead_code)]
pub fn verify_digest(doc: &Value) -> Result<(), String> {
    let Some(claimed) = doc.get("lines_digest") else {
        return Ok(());
    };
    let actual = doc
        .get("lines")
        .and_then(|l| lines_digest(l, &doc["cards"]));
    if actual.as_deref() == claimed.as_str() {
        Ok(())
    } else {
        Err("this is not the saved deck: digest mismatch; copy the get_deck result again".into())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Fixture deck whose digest was recorded from the implementation in `dumpstar-cube`
    /// before it moved here (sp-gfhhfm).
    const LINES: &str = r#"[{"card_id":"SOR-5","qty":1,"zone":"main","reserved":1},{"card_id":"C1-3","qty":2,"zone":"main","reserved":2},{"card_id":"JTL-10","qty":1,"zone":"sideboard","reserved":0}]"#;
    const CARDS: &str = r#"{"SOR-5":{"name":"Leader","subtitle":"Sub","type":"Leader","rarity":"Rare","cost":5,"game":{"aspects":["Villainy","Command"],"arenas":["Ground"],"hp":6,"epic_action":null}},"C1-3":{"name":"Base","subtitle":null,"type":"Base","rarity":"Common","cost":null,"game":{"aspects":["Vigilance"],"arenas":[],"hp":25,"epic_action":"Epic Action: Draw a card."}}}"#;
    const DIGEST: &str = "39b256e5fe319322da2c7e8a6cbf4eda2c5ae6471249ea1741894edb101eee3e";

    #[test]
    fn the_fixture_deck_digests_to_its_pinned_value() {
        let lines: Value = serde_json::from_str(LINES).unwrap();
        let cards: Value = serde_json::from_str(CARDS).unwrap();
        assert_eq!(lines_digest(&lines, &cards).as_deref(), Some(DIGEST));
        let doc = json!({"lines": lines, "cards": cards, "lines_digest": DIGEST});
        assert!(verify_digest(&doc).is_ok());
        let mut altered = doc.clone();
        altered["cards"]["SOR-5"]["cost"] = json!(4);
        assert!(verify_digest(&altered).is_err());
    }
}
