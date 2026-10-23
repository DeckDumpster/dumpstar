//! Repository for the `decks` table — physical decks built from the
//! collection. Decks carry a 3-state lifecycle: idea → ready → built.

use std::collections::HashMap;

use dumpstar_core::deck::{
    CardFacts, DeckEntry, FormatRules, Legality, LegalityReport, OwnedLine, Zone, check_deck,
};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{DbError, Result};
use crate::natsort::natural_cmp;

const COLS: &str = "d.id, d.name, d.description, d.format, d.owner, d.state, \
     d.sleeve_color, d.storage_location, d.notes, d.created_at, d.updated_at, \
     (SELECT count(*) FROM collection c WHERE c.deck_id = d.id), d.deck_group";

/// A deck, with the count of cards assigned to it.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Deck {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub format: Option<String>,
    pub owner: Option<String>,
    /// `idea` | `ready` | `built`.
    pub state: String,
    pub sleeve_color: Option<String>,
    pub storage_location: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[ts(type = "number")]
    pub card_count: i64,
    /// Optional free-text group (a cube, a set of packs).
    pub group: Option<String>,
}

/// Fields for creating a deck.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewDeck {
    pub name: String,
    pub description: Option<String>,
    pub format: Option<String>,
    pub owner: Option<String>,
    pub state: Option<String>,
    pub sleeve_color: Option<String>,
    pub storage_location: Option<String>,
    pub notes: Option<String>,
    pub group: Option<String>,
}

/// Editable deck fields. A `None` field is left unchanged.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeckEdit {
    pub name: Option<String>,
    pub description: Option<String>,
    pub format: Option<String>,
    pub owner: Option<String>,
    pub state: Option<String>,
    pub sleeve_color: Option<String>,
    pub storage_location: Option<String>,
    pub notes: Option<String>,
    pub group: Option<String>,
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Deck> {
    Ok(Deck {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        format: r.get(3)?,
        owner: r.get(4)?,
        state: r.get(5)?,
        sleeve_color: r.get(6)?,
        storage_location: r.get(7)?,
        notes: r.get(8)?,
        created_at: r.get(9)?,
        updated_at: r.get(10)?,
        card_count: r.get(11)?,
        group: r.get(12)?,
    })
}

/// Create a deck; returns its id.
pub fn create(conn: &Connection, new: &NewDeck) -> Result<i64> {
    let now = crate::clock::now_rfc3339();
    crate::deck_groups::ensure(conn, new.group.as_deref())?;
    conn.execute(
        "INSERT INTO decks \
           (name, description, format, owner, state, sleeve_color, \
            storage_location, notes, created_at, updated_at, deck_group) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?10)",
        params![
            new.name,
            new.description,
            new.format,
            new.owner,
            new.state.as_deref().unwrap_or("idea"),
            new.sleeve_color,
            new.storage_location,
            new.notes,
            now,
            new.group,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fetch one deck.
pub fn get(conn: &Connection, id: i64) -> Result<Option<Deck>> {
    Ok(conn
        .prepare(&format!("SELECT {COLS} FROM decks d WHERE d.id = ?1"))?
        .query_row([id], from_row)
        .optional()?)
}

/// List decks in natural name order (`Pack 2` before `Pack 10`), ties by id.
pub fn list(conn: &Connection) -> Result<Vec<Deck>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM decks d ORDER BY d.id"))?;
    let rows = stmt.query_map([], from_row)?;
    let mut decks: Vec<Deck> = rows.collect::<rusqlite::Result<_>>()?;
    decks.sort_by(|a, b| natural_cmp(&a.name, &b.name).then(a.id.cmp(&b.id)));
    Ok(decks)
}

/// Update editable fields. Returns whether a row changed.
pub fn update(conn: &Connection, id: i64, edit: &DeckEdit) -> Result<bool> {
    // An empty group takes the deck out of its group.
    crate::deck_groups::ensure(conn, edit.group.as_deref())?;
    let n = conn.execute(
        "UPDATE decks SET \
           name             = COALESCE(?2, name), \
           description      = COALESCE(?3, description), \
           format           = COALESCE(?4, format), \
           owner            = COALESCE(?5, owner), \
           state            = COALESCE(?6, state), \
           sleeve_color     = COALESCE(?7, sleeve_color), \
           storage_location = COALESCE(?8, storage_location), \
           notes            = COALESCE(?9, notes), \
           updated_at       = ?10, \
           deck_group       = CASE WHEN ?11 = '' THEN NULL ELSE COALESCE(?11, deck_group) END \
         WHERE id = ?1",
        params![
            id,
            edit.name,
            edit.description,
            edit.format,
            edit.owner,
            edit.state,
            edit.sleeve_color,
            edit.storage_location,
            edit.notes,
            crate::clock::now_rfc3339(),
            edit.group,
        ],
    )?;
    Ok(n > 0)
}

/// A partial update that can also clear: `None` leaves a field unchanged, `Some(None)` clears it.
#[derive(Debug, Clone, Default)]
pub struct DeckPatch {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub format: Option<Option<String>>,
    pub group: Option<Option<String>>,
    pub notes: Option<Option<String>>,
}

/// Apply a [`DeckPatch`]. Returns whether the deck exists.
pub fn patch(conn: &Connection, id: i64, p: &DeckPatch) -> Result<bool> {
    crate::deck_groups::ensure(conn, p.group.as_ref().and_then(|g| g.as_deref()))?;
    let flat = |f: &Option<Option<String>>| f.clone().flatten();
    let n = conn.execute(
        "UPDATE decks SET \
           name        = COALESCE(?2, name), \
           description = CASE WHEN ?3 THEN ?4 ELSE description END, \
           format      = CASE WHEN ?5 THEN ?6 ELSE format END, \
           deck_group  = CASE WHEN ?7 THEN ?8 ELSE deck_group END, \
           notes       = CASE WHEN ?9 THEN ?10 ELSE notes END, \
           updated_at  = ?11 \
         WHERE id = ?1",
        params![
            id,
            p.name,
            p.description.is_some(),
            flat(&p.description),
            p.format.is_some(),
            flat(&p.format),
            p.group.is_some(),
            flat(&p.group),
            p.notes.is_some(),
            flat(&p.notes),
            crate::clock::now_rfc3339(),
        ],
    )?;
    Ok(n > 0)
}

/// Set the quantity of a game card in a deck zone; `qty == 0` removes the line.
pub fn set_card(
    conn: &Connection,
    deck_id: i64,
    card_id: &str,
    zone: Zone,
    qty: u32,
) -> Result<()> {
    let zone = zone_str(zone);
    if qty == 0 {
        conn.execute(
            "DELETE FROM deck_cards WHERE deck_id=?1 AND card_id=?2 AND zone=?3",
            params![deck_id, card_id, zone],
        )?;
    } else {
        conn.execute(
            "INSERT INTO deck_cards (deck_id, card_id, zone, qty) VALUES (?1,?2,?3,?4)
             ON CONFLICT(deck_id, card_id, zone) DO UPDATE SET qty = excluded.qty",
            params![deck_id, card_id, zone, qty],
        )?;
    }
    Ok(())
}

fn zone_str(z: Zone) -> &'static str {
    match z {
        Zone::Main => "main",
        Zone::Sideboard => "sideboard",
    }
}

/// A deck's lines, as the checker takes them.
pub fn entries(conn: &Connection, deck_id: i64) -> Result<Vec<DeckEntry>> {
    let mut stmt = conn.prepare(
        "SELECT card_id, zone, qty FROM deck_cards WHERE deck_id = ?1 ORDER BY zone, card_id",
    )?;
    let rows = stmt.query_map([deck_id], |r| {
        let zone: String = r.get(1)?;
        Ok(DeckEntry {
            card_id: r.get(0)?,
            zone: if zone == "sideboard" {
                Zone::Sideboard
            } else {
                Zone::Main
            },
            qty: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Owned and missing copies for each line of a deck. Counts every owned copy
/// of the game card across all its printings, and reserves none of them.
pub fn owned_missing(conn: &Connection, deck_id: i64) -> Result<Vec<OwnedLine>> {
    let lines = entries(conn, deck_id)?;
    let mut stmt = conn.prepare(
        "SELECT count(*) FROM collection c
           JOIN shared.printings p ON p.printing_id = c.printing_id
          WHERE p.card_id = ?1 AND c.status = 'owned'",
    )?;
    let mut owned = HashMap::new();
    for l in &lines {
        if !owned.contains_key(&l.card_id) {
            let n: u32 = stmt.query_row([&l.card_id], |r| r.get(0))?;
            owned.insert(l.card_id.clone(), n);
        }
    }
    Ok(dumpstar_core::deck::owned_missing(&lines, &owned))
}

/// One line the caller asks a deck to hold.
pub type Line = DeckEntry;

/// A line the deck could not reserve: `needed` copies of the card were asked
/// for and only `available` could be had. `reserved_elsewhere` is how many of
/// the owned copies sit in other decks; they are never taken.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ShortLine {
    pub card_id: String,
    pub needed: u32,
    pub owned: u32,
    pub reserved_elsewhere: u32,
    pub available: u32,
    /// The other decks holding the owned copies, with how many each holds.
    pub held_by: Vec<DeckHold>,
    /// The next step, naming the decks to free the card from.
    pub hint: String,
}

/// A deck and how many copies of one card it holds.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeckHold {
    pub id: i64,
    pub name: String,
    pub qty: u32,
}

/// The decks holding owned copies of each card in `card_ids`, by card.
pub fn holders(conn: &Connection, card_ids: &[String]) -> Result<HashMap<String, Vec<DeckHold>>> {
    let mut stmt = conn.prepare(
        "SELECT d.id, d.name, count(*) FROM collection c \
           JOIN shared.printings p ON p.printing_id = c.printing_id \
           JOIN decks d ON d.id = c.deck_id \
          WHERE p.card_id = ?1 AND c.status = 'owned' \
          GROUP BY d.id ORDER BY d.id",
    )?;
    let mut out = HashMap::new();
    for id in card_ids {
        let mut holds: Vec<DeckHold> = stmt
            .query_map([id], |r| {
                Ok(DeckHold {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    qty: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        holds.sort_by(|a, b| natural_cmp(&a.name, &b.name).then(a.id.cmp(&b.id)));
        out.insert(id.clone(), holds);
    }
    Ok(out)
}

/// A deck line with what backs it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct DeckLineState {
    pub card_id: String,
    pub zone: Zone,
    pub qty: u32,
    /// Copies of this card (all zones) the deck holds now.
    pub reserved: u32,
}

/// Copies owned, held by decks and free, for one game card.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CardCount {
    pub card_id: String,
    pub name: String,
    pub owned: u32,
    pub reserved: u32,
    /// `owned - reserved`.
    pub available: u32,
}

/// The outcome of [`set_lines`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum SetLines {
    /// Every line reserved; the deck now holds `reserved` copies.
    Set { reserved: u32 },
    /// Nothing changed; these lines could not be filled.
    Short(Vec<ShortLine>),
}

/// Run `f` inside `BEGIN IMMEDIATE`: the write lock is taken up front, so two
/// callers racing for the last free copy are serialized and the second sees
/// the first's reservation.
pub(crate) fn immediate<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T>,
) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match f(conn) {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}

fn require_deck(conn: &Connection, id: i64) -> Result<()> {
    let found: bool = conn
        .prepare("SELECT 1 FROM decks WHERE id = ?1")?
        .exists([id])?;
    if found {
        Ok(())
    } else {
        Err(DbError::NotFound(format!("deck {id}")))
    }
}

fn log_move(
    conn: &Connection,
    copy: i64,
    from: Option<i64>,
    to: Option<i64>,
    from_binder: Option<i64>,
    note: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO movement_log (collection_id, from_binder_id, from_deck_id, to_deck_id, changed_at, note) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![copy, from_binder, from, to, crate::clock::now_rfc3339(), note],
    )?;
    Ok(())
}

/// Replace a deck's lines with `lines` in one transaction, reserving a
/// concrete owned copy for every card asked for.
///
/// Copies the deck already holds are kept where they are still wanted; the
/// rest come from copies in no deck (those in a binder last, and leaving it).
/// A copy in another deck is never taken. When any card cannot be filled
/// nothing changes and every short line is returned.
pub fn set_lines(conn: &Connection, deck_id: i64, lines: &[Line]) -> Result<SetLines> {
    let mut merged: std::collections::BTreeMap<(String, Zone), u32> = Default::default();
    for l in lines {
        if l.qty == 0 {
            continue;
        }
        *merged.entry((l.card_id.clone(), l.zone)).or_default() += l.qty;
    }
    let mut needs: std::collections::BTreeMap<&str, u32> = Default::default();
    for ((card, _), q) in &merged {
        *needs.entry(card).or_default() += q;
    }
    immediate(conn, |conn| {
        require_deck(conn, deck_id)?;
        let mut owned_stmt = conn.prepare(
            "SELECT c.id, c.deck_id, c.binder_id FROM collection c \
               JOIN shared.printings p ON p.printing_id = c.printing_id \
              WHERE p.card_id = ?1 AND c.status = 'owned' \
              ORDER BY (c.deck_id IS ?2) DESC, (c.binder_id IS NULL) DESC, c.id",
        )?;
        let mut keep: Vec<(i64, Option<i64>, Option<i64>)> = Vec::new();
        let mut short = Vec::new();
        for (card, need) in &needs {
            let rows: Vec<(i64, Option<i64>, Option<i64>)> = owned_stmt
                .query_map(params![card, deck_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let owned = rows.len() as u32;
            let elsewhere = rows
                .iter()
                .filter(|r| r.1.is_some_and(|d| d != deck_id))
                .count() as u32;
            let usable: Vec<_> = rows
                .into_iter()
                .filter(|r| r.1.is_none() || r.1 == Some(deck_id))
                .collect();
            if (usable.len() as u32) < *need {
                let held_by: Vec<DeckHold> = holders(conn, &[(*card).to_string()])?
                    .remove(*card)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|h| h.id != deck_id)
                    .collect();
                let hint = if held_by.is_empty() {
                    format!(
                        "own {} more or lower the quantity",
                        need - usable.len() as u32
                    )
                } else {
                    let names: Vec<String> = held_by
                        .iter()
                        .map(|h| format!("'{}' (id {}, qty {})", h.name, h.id, h.qty))
                        .collect();
                    format!(
                        "{card} has {owned} {}, held by deck {}: substitute a card, or remove it from the holding deck with set_deck_lines",
                        if owned == 1 { "copy" } else { "copies" },
                        names.join(", ")
                    )
                };
                short.push(ShortLine {
                    card_id: (*card).to_string(),
                    needed: *need,
                    owned,
                    reserved_elsewhere: elsewhere,
                    available: usable.len() as u32,
                    held_by,
                    hint,
                });
            } else {
                keep.extend(usable.into_iter().take(*need as usize));
            }
        }
        if !short.is_empty() {
            return Ok(SetLines::Short(short));
        }
        let keep_ids: std::collections::HashSet<i64> = keep.iter().map(|k| k.0).collect();
        let mut held = conn.prepare("SELECT id FROM collection WHERE deck_id = ?1")?;
        let held: Vec<i64> = held
            .query_map([deck_id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        for id in held.into_iter().filter(|i| !keep_ids.contains(i)) {
            conn.execute("UPDATE collection SET deck_id = NULL WHERE id = ?1", [id])?;
            log_move(conn, id, Some(deck_id), None, None, "released by set_lines")?;
        }
        for (id, deck, binder) in &keep {
            if *deck != Some(deck_id) {
                conn.execute(
                    "UPDATE collection SET deck_id = ?2, binder_id = NULL WHERE id = ?1",
                    params![id, deck_id],
                )?;
                log_move(
                    conn,
                    *id,
                    None,
                    Some(deck_id),
                    *binder,
                    "reserved by set_lines",
                )?;
            }
        }
        conn.execute("DELETE FROM deck_cards WHERE deck_id = ?1", [deck_id])?;
        for ((card, zone), qty) in &merged {
            set_card(conn, deck_id, card, *zone, *qty)?;
        }
        conn.execute(
            "UPDATE decks SET updated_at = ?2 WHERE id = ?1",
            params![deck_id, crate::clock::now_rfc3339()],
        )?;
        Ok(SetLines::Set {
            reserved: keep.len() as u32,
        })
    })
}

/// Free every copy a deck holds, keeping its lines. Returns how many copies
/// were freed.
pub fn release(conn: &Connection, deck_id: i64) -> Result<u32> {
    immediate(conn, |conn| release_in(conn, deck_id))
}

/// [`release`] inside a transaction the caller holds.
pub(crate) fn release_in(conn: &Connection, deck_id: i64) -> Result<u32> {
    require_deck(conn, deck_id)?;
    let mut stmt = conn.prepare("SELECT id FROM collection WHERE deck_id = ?1")?;
    let held: Vec<i64> = stmt
        .query_map([deck_id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    for id in &held {
        conn.execute("UPDATE collection SET deck_id = NULL WHERE id = ?1", [id])?;
        log_move(conn, *id, Some(deck_id), None, None, "released")?;
    }
    Ok(held.len() as u32)
}

/// A deck's lines with the copies backing each card.
pub fn line_states(conn: &Connection, deck_id: i64) -> Result<Vec<DeckLineState>> {
    let mut held = conn.prepare(
        "SELECT p.card_id, count(*) FROM collection c \
           JOIN shared.printings p ON p.printing_id = c.printing_id \
          WHERE c.deck_id = ?1 GROUP BY p.card_id",
    )?;
    let held: HashMap<String, u32> = held
        .query_map([deck_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(entries(conn, deck_id)?
        .into_iter()
        .map(|e| DeckLineState {
            reserved: held.get(&e.card_id).copied().unwrap_or(0),
            card_id: e.card_id,
            zone: e.zone,
            qty: e.qty,
        })
        .collect())
}

/// Owned, reserved and available copies per game card. `card_ids` limits the
/// answer; `None` lists every card with an owned copy.
pub fn card_counts(conn: &Connection, card_ids: Option<&[String]>) -> Result<Vec<CardCount>> {
    let mut stmt = conn.prepare(
        "SELECT p.card_id, cd.name, count(*), count(c.deck_id) FROM collection c \
           JOIN shared.printings p ON p.printing_id = c.printing_id \
           JOIN shared.cards cd ON cd.card_id = p.card_id \
          WHERE c.status = 'owned' GROUP BY p.card_id ORDER BY cd.name, p.card_id",
    )?;
    let rows = stmt.query_map([], |r| {
        let owned: u32 = r.get(2)?;
        let reserved: u32 = r.get(3)?;
        Ok(CardCount {
            card_id: r.get(0)?,
            name: r.get(1)?,
            owned,
            reserved,
            available: owned - reserved,
        })
    })?;
    let mut all: Vec<CardCount> = rows.collect::<rusqlite::Result<_>>()?;
    if let Some(ids) = card_ids {
        let mut out = Vec::new();
        for id in ids {
            if let Some(c) = all.iter().find(|c| &c.card_id == id) {
                out.push(c.clone());
            } else {
                let name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM shared.cards WHERE card_id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                if let Some(name) = name {
                    out.push(CardCount {
                        card_id: id.clone(),
                        name,
                        owned: 0,
                        reserved: 0,
                        available: 0,
                    });
                }
            }
        }
        all = out;
    }
    Ok(all)
}

/// The result of checking a deck against its format.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Validation {
    /// The sanctioned format the label named; `None` for a user-defined one.
    pub format_id: Option<String>,
    /// Absent for a user-defined format: no rules are checked.
    pub report: Option<LegalityReport>,
}

/// Check a deck against its sanctioned format. A label that names no format in
/// the catalog (`Premier`, `SWU Premier`, `premier`) is user-defined and gets
/// no rule checks.
pub fn validate(conn: &Connection, deck_id: i64) -> Result<Validation> {
    let deck = get(conn, deck_id)?.ok_or_else(|| DbError::NotFound(format!("deck {deck_id}")))?;
    let label = deck.format.unwrap_or_default();
    let label = label.trim().to_lowercase();
    let label = label.strip_prefix("swu ").unwrap_or(&label);
    let format_id: Option<String> = conn
        .query_row(
            "SELECT format_id FROM shared.formats WHERE lower(format_id) = ?1 OR lower(name) = ?1",
            [label],
            |r| r.get(0),
        )
        .optional()?;
    let Some(fid) = format_id else {
        return Ok(Validation {
            format_id: None,
            report: None,
        });
    };
    let mut stmt =
        conn.prepare("SELECT rule_key, rule_value FROM shared.format_rules WHERE format_id = ?1")?;
    let map = stmt
        .query_map([&fid], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let rules =
        FormatRules::from_map(&map).map_err(|e| DbError::Invalid(format!("format {fid}: {e}")))?;
    let lines = entries(conn, deck_id)?;
    let mut facts = HashMap::new();
    let mut f = conn.prepare(
        "SELECT c.card_type, c.aspects, l.status FROM shared.cards c \
           LEFT JOIN shared.card_legality l ON l.card_id = c.card_id AND l.format_id = ?2 \
          WHERE c.card_id = ?1",
    )?;
    for l in &lines {
        let row = f
            .query_row(params![l.card_id, fid], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                ))
            })
            .optional()?;
        if let Some((ty, aspects, status)) = row {
            facts.insert(
                l.card_id.clone(),
                CardFacts {
                    card_type: ty.unwrap_or_default(),
                    aspects: serde_json::from_str(&aspects)?,
                    legality: status
                        .as_deref()
                        .and_then(Legality::parse)
                        .unwrap_or_default(),
                },
            );
        }
    }
    Ok(Validation {
        format_id: Some(fid),
        report: Some(check_deck(&rules, &lines, &facts)),
    })
}

/// Delete a deck. Its cards are un-assigned (collection.deck_id is
/// `ON DELETE SET NULL`), never deleted.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    immediate(conn, |conn| delete_in(conn, id))
}

/// [`delete`] inside a transaction the caller holds.
pub(crate) fn delete_in(conn: &Connection, id: i64) -> Result<bool> {
    // The move log names decks without a cascade; the history of the
    // copies stays, minus the reference to the deck that is going.
    conn.execute(
        "UPDATE movement_log SET from_deck_id = NULL WHERE from_deck_id = ?1",
        [id],
    )?;
    conn.execute(
        "UPDATE movement_log SET to_deck_id = NULL WHERE to_deck_id = ?1",
        [id],
    )?;
    Ok(conn.execute("DELETE FROM decks WHERE id = ?1", [id])? > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_user, open_shared};

    fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        open_shared(&shared).unwrap();
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    #[test]
    fn list_orders_names_naturally() {
        let (_d, conn) = conn();
        for name in ["Pack 1", "Pack 10", "Pack 2"] {
            create(
                &conn,
                &NewDeck {
                    name: name.into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
        let names: Vec<String> = list(&conn).unwrap().into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["Pack 1", "Pack 2", "Pack 10"]);
    }

    #[test]
    fn crud_round_trip() {
        let (_d, conn) = conn();
        let id = create(
            &conn,
            &NewDeck {
                name: "Alice's Luke deck".into(),
                owner: Some("Alice".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let d = get(&conn, id).unwrap().unwrap();
        assert_eq!(d.owner.as_deref(), Some("Alice"));
        assert_eq!(d.state, "idea"); // default lifecycle state
        assert_eq!(d.card_count, 0);

        assert!(
            update(
                &conn,
                id,
                &DeckEdit {
                    state: Some("built".into()),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        assert_eq!(get(&conn, id).unwrap().unwrap().state, "built");

        assert!(delete(&conn, id).unwrap());
        assert!(get(&conn, id).unwrap().is_none());
    }

    #[test]
    fn rejects_invalid_state() {
        let (_d, conn) = conn();
        let err = create(
            &conn,
            &NewDeck {
                name: "Bad".into(),
                state: Some("nonsense".into()),
                ..Default::default()
            },
        );
        assert!(
            err.is_err(),
            "the state CHECK constraint should reject this"
        );
    }

    #[test]
    fn owned_missing_counts_the_card_across_printings_and_reserves_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            crate::testkit::set(&c, "SOR", "Spark of Rebellion");
            crate::testkit::card(&c, "C1", "Card One");
            // Standard and Hyperspace print under different numbers.
            crate::testkit::printing(&c, "C1-std", "C1", "SOR", "1", "Standard", "Normal");
            crate::testkit::printing(&c, "C1-hyper", "C1", "SOR", "251", "Hyperspace", "Normal");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        for p in ["C1-std", "C1-hyper"] {
            conn.execute(
                "INSERT INTO collection (printing_id, acquired_at, source) VALUES (?1,'2026-10-13','manual_id')",
                [p],
            )
            .unwrap();
        }
        let a = create(
            &conn,
            &NewDeck {
                name: "A".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let b = create(
            &conn,
            &NewDeck {
                name: "B".into(),
                ..Default::default()
            },
        )
        .unwrap();
        for d in [a, b] {
            set_card(&conn, d, "C1", Zone::Main, 3).unwrap();
        }
        set_card(&conn, a, "C2", Zone::Main, 1).unwrap();
        for d in [a, b] {
            let l = owned_missing(&conn, d).unwrap();
            let c1 = l.iter().find(|l| l.card_id == "C1").unwrap();
            // Two printings count as two copies of one card; deck B still sees both.
            assert_eq!((c1.needed, c1.owned, c1.missing), (3, 2, 1));
        }
        let c2 = owned_missing(&conn, a)
            .unwrap()
            .into_iter()
            .find(|l| l.card_id == "C2")
            .unwrap();
        assert_eq!((c2.owned, c2.missing), (0, 1));
        set_card(&conn, a, "C2", Zone::Main, 0).unwrap();
        assert_eq!(entries(&conn, a).unwrap().len(), 1);
    }

    /// A tenant with `n` owned copies of card C1 (Standard printing) and two decks.
    fn stocked(
        n: usize,
    ) -> (
        tempfile::TempDir,
        std::path::PathBuf,
        std::path::PathBuf,
        Connection,
        i64,
        i64,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            crate::testkit::set(&c, "SOR", "Spark of Rebellion");
            crate::testkit::card(&c, "C1", "Card One");
            crate::testkit::printing(&c, "C1-std", "C1", "SOR", "1", "Standard", "Normal");
        }
        let user = dir.path().join("collection.sqlite");
        let conn = connect_user(&user, &shared).unwrap();
        for _ in 0..n {
            conn.execute(
                "INSERT INTO collection (printing_id, acquired_at, source) VALUES ('C1-std','2026-10-13','manual_id')",
                [],
            )
            .unwrap();
        }
        let mk = |name: &str| {
            create(
                &conn,
                &NewDeck {
                    name: name.into(),
                    ..Default::default()
                },
            )
            .unwrap()
        };
        let (a, b) = (mk("A"), mk("B"));
        (dir, shared, user, conn, a, b)
    }

    fn line(card: &str, qty: u32) -> Line {
        Line {
            card_id: card.into(),
            zone: Zone::Main,
            qty,
        }
    }

    fn counts(conn: &Connection) -> CardCount {
        card_counts(conn, Some(&["C1".to_string()]))
            .unwrap()
            .remove(0)
    }

    #[test]
    fn set_lines_reserves_and_refuses_over_reservation_naming_the_line() {
        let (_d, _s, _u, conn, a, b) = stocked(3);
        assert_eq!(
            set_lines(&conn, a, &[line("C1", 2)]).unwrap(),
            SetLines::Set { reserved: 2 }
        );
        let c = counts(&conn);
        assert_eq!((c.owned, c.reserved, c.available), (3, 2, 1));

        // B wants 2 of the 3 but only 1 is free: refused, A untouched.
        let r = set_lines(&conn, b, &[line("C1", 2), line("GHOST", 1)]).unwrap();
        let SetLines::Short(short) = r else {
            panic!("expected short")
        };
        assert_eq!(short.len(), 2);
        assert_eq!(
            short[0],
            ShortLine {
                card_id: "C1".into(),
                needed: 2,
                owned: 3,
                reserved_elsewhere: 2,
                available: 1,
                held_by: vec![DeckHold {
                    id: a,
                    name: "A".into(),
                    qty: 2
                }],
                hint: format!(
                    "C1 has 3 copies, held by deck 'A' (id {a}, qty 2): substitute a card, or remove it from the holding deck with set_deck_lines"
                ),
            }
        );
        assert_eq!(
            holders(&conn, &["C1".to_string()]).unwrap()["C1"],
            vec![DeckHold {
                id: a,
                name: "A".into(),
                qty: 2
            }]
        );
        assert_eq!(short[1].card_id, "GHOST");
        assert_eq!(counts(&conn).reserved, 2);
        assert!(entries(&conn, b).unwrap().is_empty());

        // Replacing A's own lines may reuse its copies.
        assert_eq!(
            set_lines(&conn, a, &[line("C1", 3)]).unwrap(),
            SetLines::Set { reserved: 3 }
        );
        assert_eq!(counts(&conn).available, 0);
        // Shrinking releases the surplus.
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        assert_eq!(counts(&conn).available, 2);
        // Main + sideboard of one card draw on the same copies.
        let both = [
            line("C1", 2),
            Line {
                card_id: "C1".into(),
                zone: Zone::Sideboard,
                qty: 2,
            },
        ];
        assert!(matches!(
            set_lines(&conn, b, &both).unwrap(),
            SetLines::Short(_)
        ));
    }

    #[test]
    fn release_and_delete_free_the_copies() {
        let (_d, _s, _u, conn, a, b) = stocked(2);
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        set_lines(&conn, b, &[line("C1", 1)]).unwrap();
        assert_eq!(counts(&conn).available, 0);
        assert_eq!(release(&conn, a).unwrap(), 1);
        assert_eq!(counts(&conn).available, 1);
        assert_eq!(line_states(&conn, a).unwrap()[0].reserved, 0);
        assert!(delete(&conn, b).unwrap());
        assert_eq!(counts(&conn).available, 2);
        assert!(matches!(release(&conn, 999), Err(DbError::NotFound(_))));
        assert!(matches!(
            set_lines(&conn, 999, &[]),
            Err(DbError::NotFound(_))
        ));
    }

    #[test]
    fn a_binder_copy_is_used_last_and_leaves_its_binder() {
        let (_d, _s, _u, conn, a, _b) = stocked(2);
        conn.execute(
            "INSERT INTO binders (name, created_at, updated_at) VALUES ('T','x','x')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE collection SET binder_id = 1 WHERE id = 1", [])
            .unwrap();
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        let in_deck: i64 = conn
            .query_row("SELECT id FROM collection WHERE deck_id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(in_deck, 2, "the binder copy is kept for when it is needed");
        set_lines(&conn, a, &[line("C1", 2)]).unwrap();
        let binder: Option<i64> = conn
            .query_row("SELECT binder_id FROM collection WHERE id = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(binder, None);
    }

    #[test]
    fn two_racing_set_lines_for_the_last_copy_have_exactly_one_winner() {
        let (_d, shared, user, _conn, a, b) = stocked(1);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = [a, b]
            .into_iter()
            .map(|deck| {
                let (shared, user, barrier) = (shared.clone(), user.clone(), barrier.clone());
                std::thread::spawn(move || {
                    let conn = connect_user(&user, &shared).unwrap();
                    barrier.wait();
                    set_lines(&conn, deck, &[line("C1", 1)]).unwrap()
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results
            .iter()
            .filter(|r| matches!(r, SetLines::Set { .. }))
            .count();
        assert_eq!(wins, 1, "{results:?}");
    }

    #[test]
    fn user_defined_formats_get_no_rule_checks_and_premier_does() {
        let (_d, _s, _u, conn, a, _b) = stocked(1);
        update(
            &conn,
            a,
            &DeckEdit {
                format: Some("Jumpstart cube".into()),
                group: Some("cube-1".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            get(&conn, a).unwrap().unwrap().group.as_deref(),
            Some("cube-1")
        );
        let v = validate(&conn, a).unwrap();
        assert!(v.format_id.is_none() && v.report.is_none());
        update(
            &conn,
            a,
            &DeckEdit {
                format: Some("SWU Premier".into()),
                ..Default::default()
            },
        )
        .unwrap();
        // The test catalog carries no formats; the label then names none.
        assert!(validate(&conn, a).unwrap().report.is_none());
    }
}
