//! Catalog fixtures for this crate's tests: the Star Wars: Unlimited rows a
//! test needs, written the way the catalog apply job writes them. Pass the
//! read-write catalog connection (`open_shared`).

use rusqlite::{Connection, params};

/// A set, once. `release_date` defaults to a fixed day; `sort_order` follows
/// insertion.
pub(crate) fn set(c: &Connection, code: &str, name: &str) {
    c.execute(
        "INSERT OR IGNORE INTO sets (set_code, name, release_date, sort_order) \
         VALUES (?1, ?2, '2024-03-08', (SELECT COUNT(*) FROM sets))",
        params![code, name],
    )
    .unwrap();
}

/// A set with an explicit release date.
pub(crate) fn set_on(c: &Connection, code: &str, name: &str, release_date: &str) {
    c.execute(
        "INSERT OR IGNORE INTO sets (set_code, name, release_date, sort_order) \
         VALUES (?1, ?2, ?3, (SELECT COUNT(*) FROM sets))",
        params![code, name, release_date],
    )
    .unwrap();
}

/// A game card, once. A Unit with no stats.
pub(crate) fn card(c: &Connection, card_id: &str, name: &str) {
    c.execute(
        "INSERT OR IGNORE INTO cards (card_id, name, card_type) VALUES (?1, ?2, 'Unit')",
        params![card_id, name],
    )
    .unwrap();
}

/// A game card with its stats.
#[allow(clippy::too_many_arguments)]
pub(crate) fn card_full(
    c: &Connection,
    card_id: &str,
    name: &str,
    subtitle: Option<&str>,
    card_type: &str,
    cost: Option<i64>,
    power: Option<i64>,
    hp: Option<i64>,
    aspects: &str,
    traits: &str,
    arenas: &str,
    rules_text: Option<&str>,
) {
    c.execute(
        "INSERT INTO cards (card_id, name, subtitle, card_type, cost, power, hp, aspects, \
                            traits, arenas, rules_text) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            card_id, name, subtitle, card_type, cost, power, hp, aspects, traits, arenas,
            rules_text
        ],
    )
    .unwrap();
    c.execute(
        "INSERT INTO card_search (card_id, name, subtitle, rules_text, traits) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![card_id, name, subtitle, rules_text, traits],
    )
    .unwrap();
}

/// One printing of an existing card in an existing set.
pub(crate) fn printing(
    c: &Connection,
    printing_id: &str,
    card_id: &str,
    set_code: &str,
    number: &str,
    variant: &str,
    finish: &str,
) {
    c.execute(
        "INSERT INTO printings (printing_id, card_id, set_code, number, variant, finish) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![printing_id, card_id, set_code, number, variant, finish],
    )
    .unwrap();
}

/// The shorthand most tests want: creates the set (named after its code), the
/// card `{set}-{number}`, and its Standard Normal printing
/// `{set}-{number}-normal`. Returns that printing's id.
pub(crate) fn simple(c: &Connection, set_code: &str, number: &str, name: &str) -> String {
    set(c, set_code, set_code);
    let card_id = format!("{set_code}-{number}");
    card(c, &card_id, name);
    let printing_id = format!("{card_id}-normal");
    printing(
        c,
        &printing_id,
        &card_id,
        set_code,
        number,
        "Standard",
        "Normal",
    );
    printing_id
}

/// The Foil of a [`simple`] printing: same set and number, `{set}-{number}-foil`.
pub(crate) fn foil_of(c: &Connection, set_code: &str, number: &str) -> String {
    let card_id = format!("{set_code}-{number}");
    let printing_id = format!("{card_id}-foil");
    printing(
        c,
        &printing_id,
        &card_id,
        set_code,
        number,
        "Standard",
        "Foil",
    );
    printing_id
}

/// A market quote for a printing on a day, source `tcgcsv`.
pub(crate) fn price(c: &Connection, printing_id: &str, day: &str, market: f64) {
    c.execute(
        "INSERT INTO prices (printing_id, source, observed_on, market) \
         VALUES (?1, 'tcgcsv', ?2, ?3)",
        params![printing_id, day, market],
    )
    .unwrap();
}
