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
     (SELECT count(*) FROM collection c WHERE c.deck_id = d.id), d.deck_group, d.precon_id";

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
    /// The preconstructed deck this one was added from; `None` for any other.
    pub precon_id: Option<String>,
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
        precon_id: r.get(13)?,
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

/// Which printings of a card a line may be filled from. The default names
/// nothing: any owned copy of the card will do.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Pick {
    pub printing_id: Option<String>,
    pub finish: Option<String>,
    pub variant: Option<String>,
}

impl Pick {
    /// 0 for a named printing, then finish and variant together, then one of
    /// them, then nothing: demands are filled in this order so a narrow one
    /// is never starved by a broad one.
    fn specificity(&self) -> u8 {
        if self.printing_id.is_some() {
            0
        } else {
            match (self.finish.is_some(), self.variant.is_some()) {
                (true, true) => 1,
                (false, false) => 3,
                _ => 2,
            }
        }
    }

    fn matches(&self, printing: &str, finish: &str, variant: &str) -> bool {
        self.printing_id.as_deref().is_none_or(|p| p == printing)
            && self.finish.as_deref().is_none_or(|f| f == finish)
            && self.variant.as_deref().is_none_or(|v| v == variant)
    }

    /// The selector as prose, e.g. `Foil Hyperspace`; empty when none.
    fn label(&self) -> String {
        [&self.printing_id, &self.finish, &self.variant]
            .into_iter()
            .flatten()
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// One line the caller asks a deck to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub card_id: String,
    pub zone: Zone,
    pub qty: u32,
    pub pick: Pick,
}

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
    /// The selector this line asked for, when it asked for one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printing_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
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
    /// Those copies, one per `reserved`, in collection order: which printing and finish.
    pub copies: Vec<HeldCopy>,
}

/// One physical copy a deck holds, with the printing it is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct HeldCopy {
    pub copy_id: i64,
    pub printing_id: String,
    pub set: String,
    pub number: String,
    pub variant: String,
    pub finish: String,
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
    immediate(conn, |conn| set_lines_in(conn, deck_id, lines))
}

/// [`set_lines`] inside a transaction the caller holds.
pub(crate) fn set_lines_in(conn: &Connection, deck_id: i64, lines: &[Line]) -> Result<SetLines> {
    let mut merged: std::collections::BTreeMap<(String, Zone), u32> = Default::default();
    // Per card, the copies wanted for each selector.
    let mut demands: std::collections::BTreeMap<&str, std::collections::BTreeMap<&Pick, u32>> =
        Default::default();
    for l in lines {
        if l.qty == 0 {
            continue;
        }
        *merged.entry((l.card_id.clone(), l.zone)).or_default() += l.qty;
        *demands
            .entry(&l.card_id)
            .or_default()
            .entry(&l.pick)
            .or_default() += l.qty;
    }
    require_deck(conn, deck_id)?;
    let mut owned_stmt = conn.prepare(
        "SELECT c.id, c.deck_id, c.binder_id, p.printing_id, p.finish, p.variant FROM collection c \
           JOIN shared.printings p ON p.printing_id = c.printing_id \
          WHERE p.card_id = ?1 AND c.status = 'owned' \
          ORDER BY (c.deck_id IS ?2) DESC, (c.binder_id IS NULL) DESC, c.id",
    )?;
    type Row = (i64, Option<i64>, Option<i64>, String, String, String);
    let mut keep: Vec<(i64, Option<i64>, Option<i64>)> = Vec::new();
    let mut short = Vec::new();
    for (card, picks) in &demands {
        let rows: Vec<Row> = owned_stmt
            .query_map(params![card, deck_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let owned = rows.len() as u32;
        let elsewhere = rows
            .iter()
            .filter(|r| r.1.is_some_and(|d| d != deck_id))
            .count() as u32;
        let usable: Vec<Row> = rows
            .into_iter()
            .filter(|r| r.1.is_none() || r.1 == Some(deck_id))
            .collect();
        let mut taken = vec![false; usable.len()];
        let mut order: Vec<(&&Pick, &u32)> = picks.iter().collect();
        order.sort_by_key(|(p, _)| p.specificity());
        for (pick, need) in order {
            let mut got = 0;
            for (i, r) in usable.iter().enumerate() {
                if got == *need {
                    break;
                }
                if !taken[i] && pick.matches(&r.3, &r.4, &r.5) {
                    taken[i] = true;
                    got += 1;
                    keep.push((r.0, r.1, r.2));
                }
            }
            if got == *need {
                continue;
            }
            let label = pick.label();
            let held_by: Vec<DeckHold> = holders(conn, &[(*card).to_string()])?
                .remove(*card)
                .unwrap_or_default()
                .into_iter()
                .filter(|h| h.id != deck_id)
                .collect();
            let hint = if held_by.is_empty() {
                let kind = if label.is_empty() {
                    String::new()
                } else {
                    format!(" {label}")
                };
                format!("own {} more{kind} or lower the quantity", need - got)
            } else {
                let names: Vec<String> = held_by
                    .iter()
                    .map(|h| format!("'{}' (id {}, qty {})", h.name, h.id, h.qty))
                    .collect();
                let wanted = if label.is_empty() {
                    String::new()
                } else {
                    format!(" ({label} wanted)")
                };
                format!(
                    "{card} has {owned} {}{wanted}, held by deck {}: substitute a card, or remove it from the holding deck with set_deck_lines",
                    if owned == 1 { "copy" } else { "copies" },
                    names.join(", ")
                )
            };
            short.push(ShortLine {
                card_id: (*card).to_string(),
                needed: *need,
                owned,
                reserved_elsewhere: elsewhere,
                available: got,
                printing_id: pick.printing_id.clone(),
                finish: pick.finish.clone(),
                variant: pick.variant.clone(),
                held_by,
                hint,
            });
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
}

/// Free `out_copy`, which the deck holds, and reserve `in_copy`, a free owned
/// copy of the same card, in its place. The deck's lines are untouched. Either
/// both moves happen or, on refusal, neither.
pub fn swap_copy(conn: &Connection, deck_id: i64, out_copy: i64, in_copy: i64) -> Result<()> {
    type Row = (Option<i64>, String, Option<i64>, String);
    immediate(conn, |conn| {
        require_deck(conn, deck_id)?;
        let read = |id: i64| -> Result<Row> {
            conn.query_row(
                "SELECT c.deck_id, c.status, c.binder_id, p.card_id FROM collection c \
                   JOIN shared.printings p ON p.printing_id = c.printing_id \
                  WHERE c.id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => DbError::NotFound(format!("copy {id}")),
                e => e.into(),
            })
        };
        let (out_deck, _, _, out_card) = read(out_copy)?;
        let (in_deck, in_status, in_binder, in_card) = read(in_copy)?;
        if out_deck != Some(deck_id) {
            return Err(DbError::Conflict(format!(
                "copy {out_copy} is not in deck {deck_id}"
            )));
        }
        if in_status != "owned" {
            return Err(DbError::Conflict(format!(
                "copy {in_copy} is not owned (status {in_status})"
            )));
        }
        if let Some(held) = in_deck {
            let name: String = conn
                .query_row("SELECT name FROM decks WHERE id = ?1", [held], |r| r.get(0))
                .unwrap_or_default();
            return Err(DbError::Conflict(format!(
                "copy {in_copy} is held by deck '{name}' (id {held})"
            )));
        }
        if out_card != in_card {
            return Err(DbError::Conflict(format!(
                "copy {out_copy} is {out_card} but copy {in_copy} is {in_card}: a swap needs the same card"
            )));
        }
        conn.execute(
            "UPDATE collection SET deck_id = NULL WHERE id = ?1",
            [out_copy],
        )?;
        conn.execute(
            "UPDATE collection SET deck_id = ?2, binder_id = NULL WHERE id = ?1",
            params![in_copy, deck_id],
        )?;
        log_move(
            conn,
            out_copy,
            Some(deck_id),
            None,
            None,
            "swapped out by swap_deck_copy",
        )?;
        log_move(
            conn,
            in_copy,
            None,
            Some(deck_id),
            in_binder,
            "swapped in by swap_deck_copy",
        )?;
        conn.execute(
            "UPDATE decks SET updated_at = ?2 WHERE id = ?1",
            params![deck_id, crate::clock::now_rfc3339()],
        )?;
        Ok(())
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
    let mut stmt = conn.prepare(
        "SELECT p.card_id, c.id, p.printing_id, p.set_code, p.number, p.variant, p.finish \
           FROM collection c \
           JOIN shared.printings p ON p.printing_id = c.printing_id \
          WHERE c.deck_id = ?1 ORDER BY c.id",
    )?;
    let mut held: HashMap<String, Vec<HeldCopy>> = HashMap::new();
    for row in stmt.query_map([deck_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            HeldCopy {
                copy_id: r.get(1)?,
                printing_id: r.get(2)?,
                set: r.get(3)?,
                number: r.get(4)?,
                variant: r.get(5)?,
                finish: r.get(6)?,
            },
        ))
    })? {
        let (card_id, copy) = row?;
        held.entry(card_id).or_default().push(copy);
    }
    Ok(entries(conn, deck_id)?
        .into_iter()
        .map(|e| {
            let copies = held.get(&e.card_id).cloned().unwrap_or_default();
            DeckLineState {
                reserved: copies.len() as u32,
                copies,
                card_id: e.card_id,
                zone: e.zone,
                qty: e.qty,
            }
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
            pick: Pick::default(),
        }
    }

    fn counts(conn: &Connection) -> CardCount {
        card_counts(conn, Some(&["C1".to_string()]))
            .unwrap()
            .remove(0)
    }

    /// `stocked(3)` plus a second card C2 with one owned copy; copy ids of C1 are 1..=3, C2's is 4.
    fn stocked_with_c2() -> (tempfile::TempDir, Connection, i64, i64) {
        let (d, shared, _u, conn, a, b) = stocked(3);
        {
            let c = open_shared(&shared).unwrap();
            crate::testkit::card(&c, "C2", "Card Two");
            crate::testkit::printing(&c, "C2-std", "C2", "SOR", "2", "Standard", "Normal");
        }
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source) VALUES ('C2-std','2026-10-13','manual_id')",
            [],
        )
        .unwrap();
        (d, conn, a, b)
    }

    fn deck_ids(conn: &Connection) -> Vec<Option<i64>> {
        conn.prepare("SELECT deck_id FROM collection ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn swap_copy_moves_the_reservation_and_leaves_the_lines() {
        let (_d, conn, a, _b) = stocked_with_c2();
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        let out: i64 = conn
            .query_row("SELECT id FROM collection WHERE deck_id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        let free: i64 = conn
            .query_row(
                "SELECT id FROM collection WHERE deck_id IS NULL AND id <= 3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let lines_before = entries(&conn, a).unwrap();
        let logs = |c: &Connection| -> i64 {
            c.query_row("SELECT COUNT(*) FROM movement_log", [], |r| r.get(0))
                .unwrap()
        };
        let log_before = logs(&conn);
        let updated_before: String = conn
            .query_row("SELECT updated_at FROM decks WHERE id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();

        swap_copy(&conn, a, out, free).unwrap();

        let held = |id: i64| -> Option<i64> {
            conn.query_row("SELECT deck_id FROM collection WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
        };
        assert_eq!((held(out), held(free)), (None, Some(a)));
        assert_eq!(logs(&conn), log_before + 2);
        assert_eq!(entries(&conn, a).unwrap(), lines_before);

        let (note, from_deck, to_deck): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT note, from_deck_id, to_deck_id FROM movement_log WHERE collection_id = ?1 ORDER BY id DESC LIMIT 1",
                [out],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (note.as_str(), from_deck, to_deck),
            ("swapped out by swap_deck_copy", Some(a), None)
        );
        let (note, from_deck, to_deck): (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT note, from_deck_id, to_deck_id FROM movement_log WHERE collection_id = ?1 ORDER BY id DESC LIMIT 1",
                [free],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (note.as_str(), from_deck, to_deck),
            ("swapped in by swap_deck_copy", None, Some(a))
        );
        let updated: String = conn
            .query_row("SELECT updated_at FROM decks WHERE id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        assert_ne!(updated, updated_before);

        // The same lines keep the swapped-in copy and do not take the old one back.
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        assert_eq!((held(out), held(free)), (None, Some(a)));
    }

    #[test]
    fn swap_copy_takes_a_binder_copy_out_of_its_binder_and_refuses_one_not_owned() {
        let (_d, conn, a, _b) = stocked_with_c2();
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        let out: i64 = conn
            .query_row("SELECT id FROM collection WHERE deck_id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        let free: i64 = conn
            .query_row(
                "SELECT id FROM collection WHERE deck_id IS NULL AND id <= 3 ORDER BY id",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let sold: i64 = conn
            .query_row(
                "SELECT id FROM collection WHERE deck_id IS NULL AND id <= 3 AND id <> ?1",
                [free],
                |r| r.get(0),
            )
            .unwrap();
        conn.execute(
            "INSERT INTO binders (id, name, created_at, updated_at) VALUES (7, 'B', 'x', 'x')",
            [],
        )
        .unwrap();
        conn.execute("UPDATE collection SET binder_id = 7 WHERE id = ?1", [free])
            .unwrap();
        conn.execute(
            "UPDATE collection SET status = 'sold' WHERE id = ?1",
            [sold],
        )
        .unwrap();

        let err = swap_copy(&conn, a, out, sold).unwrap_err();
        assert!(matches!(err, DbError::Conflict(_)), "{err:?}");
        assert_eq!(deck_ids(&conn)[(out - 1) as usize], Some(a));

        swap_copy(&conn, a, out, free).unwrap();
        let (binder, from_binder): (Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT c.binder_id, (SELECT from_binder_id FROM movement_log WHERE collection_id = c.id ORDER BY id DESC LIMIT 1) \
                 FROM collection c WHERE c.id = ?1",
                [free],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((binder, from_binder), (None, Some(7)));
    }

    #[test]
    fn swap_copy_refusals_change_nothing() {
        let (_d, conn, a, b) = stocked_with_c2();
        set_lines(&conn, a, &[line("C1", 1)]).unwrap();
        set_lines(&conn, b, &[line("C1", 1)]).unwrap();
        let in_a: i64 = conn
            .query_row("SELECT id FROM collection WHERE deck_id = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        let in_b: i64 = conn
            .query_row("SELECT id FROM collection WHERE deck_id = ?1", [b], |r| {
                r.get(0)
            })
            .unwrap();
        let free: i64 = conn
            .query_row(
                "SELECT id FROM collection WHERE deck_id IS NULL AND id <= 3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let before = deck_ids(&conn);
        for (out, inn) in [
            (in_a, 4),
            (free, free),
            (in_b, free),
            (in_a, in_b),
            (in_a, 99),
        ] {
            let err = swap_copy(&conn, a, out, inn).unwrap_err();
            assert!(
                matches!(err, DbError::Conflict(_) | DbError::NotFound(_)),
                "{out}->{inn}: {err:?}"
            );
            assert_eq!(deck_ids(&conn), before, "{out}->{inn}");
        }
        assert!(matches!(
            swap_copy(&conn, 999, in_a, free),
            Err(DbError::NotFound(_))
        ));
    }

    fn pick_line(qty: u32, printing: Option<&str>, finish: Option<&str>) -> Line {
        Line {
            pick: Pick {
                printing_id: printing.map(Into::into),
                finish: finish.map(Into::into),
                variant: None,
            },
            ..line("C1", qty)
        }
    }

    /// `stocked(2)` (Normal copies 1 and 2) plus a Foil printing `C1-foil` with one owned copy, id 3.
    fn stocked_with_foil() -> (Connection, i64, i64) {
        let (d, shared, _u, conn, a, b) = stocked(2);
        std::mem::forget(d);
        crate::testkit::printing(
            &open_shared(&shared).unwrap(),
            "C1-foil",
            "C1",
            "SOR",
            "1",
            "Standard",
            "Foil",
        );
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source) VALUES ('C1-foil','2026-10-13','manual_id')",
            [],
        )
        .unwrap();
        (conn, a, b)
    }

    fn held_ids(conn: &Connection, deck: i64) -> Vec<i64> {
        conn.prepare("SELECT id FROM collection WHERE deck_id = ?1 ORDER BY id")
            .unwrap()
            .query_map([deck], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn one_card_on_two_lines_reserves_the_foil_and_a_normal_copy() {
        let (conn, a, _b) = stocked_with_foil();
        let lines = [
            pick_line(1, Some("C1-foil"), None),
            pick_line(1, None, Some("Normal")),
        ];
        assert_eq!(
            set_lines(&conn, a, &lines).unwrap(),
            SetLines::Set { reserved: 2 }
        );
        assert_eq!(held_ids(&conn, a), vec![1, 3]);
    }

    #[test]
    fn finish_and_variant_together_match_only_that_printing_and_name_both_when_short() {
        let (_d, shared, _u, conn, a, _b) = stocked(2);
        crate::testkit::printing(
            &open_shared(&shared).unwrap(),
            "C1-hs-foil",
            "C1",
            "SOR",
            "1h",
            "Hyperspace",
            "Foil",
        );
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source) VALUES ('C1-hs-foil','2026-10-13','manual_id')",
            [],
        )
        .unwrap();
        let pick = |finish: &str, variant: &str, qty| Line {
            pick: Pick {
                printing_id: None,
                finish: Some(finish.into()),
                variant: Some(variant.into()),
            },
            ..line("C1", qty)
        };
        assert_eq!(
            set_lines(&conn, a, &[pick("Foil", "Hyperspace", 1)]).unwrap(),
            SetLines::Set { reserved: 1 }
        );
        assert_eq!(held_ids(&conn, a), vec![3]);
        let SetLines::Short(short) = set_lines(&conn, a, &[pick("Foil", "Hyperspace", 2)]).unwrap()
        else {
            panic!("expected short")
        };
        assert_eq!(short[0].variant.as_deref(), Some("Hyperspace"));
        assert_eq!(short[0].finish.as_deref(), Some("Foil"));
        assert_eq!(short[0].available, 1);
    }

    #[test]
    fn a_selector_with_too_few_matching_copies_refuses_and_moves_nothing() {
        let (conn, a, _b) = stocked_with_foil();
        let before = deck_ids(&conn);
        let r = set_lines(&conn, a, &[pick_line(2, None, Some("Foil"))]).unwrap();
        let SetLines::Short(short) = r else {
            panic!("expected short")
        };
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].finish.as_deref(), Some("Foil"));
        assert_eq!(short[0].printing_id, None);
        assert_eq!((short[0].needed, short[0].available), (2, 1));
        assert_eq!(short[0].hint, "own 1 more Foil or lower the quantity");
        assert_eq!(deck_ids(&conn), before);
    }

    #[test]
    fn rerunning_set_lines_keeps_the_copies_the_deck_already_holds() {
        let (conn, a, b) = stocked_with_foil();
        // B takes copy 1, A gets 2 and the foil 3; B then lets go, leaving a
        // free Normal copy with a lower id than the one A holds.
        set_lines(&conn, b, &[line("C1", 1)]).unwrap();
        let lines = [
            pick_line(1, Some("C1-foil"), None),
            pick_line(1, None, Some("Normal")),
        ];
        set_lines(&conn, a, &lines).unwrap();
        set_lines(&conn, b, &[]).unwrap();
        assert_eq!(held_ids(&conn, a), vec![2, 3]);
        set_lines(&conn, a, &lines).unwrap();
        assert_eq!(held_ids(&conn, a), vec![2, 3]);
        set_lines(&conn, a, &[line("C1", 2)]).unwrap();
        assert_eq!(held_ids(&conn, a), vec![2, 3]);
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
                printing_id: None,
                finish: None,
                variant: None,
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
                pick: Pick::default(),
            },
        ];
        assert!(matches!(
            set_lines(&conn, b, &both).unwrap(),
            SetLines::Short(_)
        ));
    }

    #[test]
    fn line_states_name_the_reserved_copies_with_their_finish() {
        let (_d, shared, _u, conn, a, _b) = stocked(1);
        {
            let c = open_shared(&shared).unwrap();
            crate::testkit::printing(&c, "C1-foil", "C1", "SOR", "1", "Hyperspace", "Foil");
        }
        conn.execute(
            "INSERT INTO collection (printing_id, acquired_at, source) VALUES ('C1-foil','2026-10-13','manual_id')",
            [],
        )
        .unwrap();
        assert!(matches!(
            set_lines(&conn, a, &[line("C1", 2)]).unwrap(),
            SetLines::Set { .. }
        ));
        let ids: Vec<i64> = conn
            .prepare("SELECT id FROM collection WHERE deck_id = ?1 ORDER BY id")
            .unwrap()
            .query_map([a], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let l = &line_states(&conn, a).unwrap()[0];
        assert_eq!(l.reserved, 2);
        assert_eq!(l.copies.iter().map(|c| c.copy_id).collect::<Vec<_>>(), ids);
        let got: Vec<(&str, &str, &str)> = l
            .copies
            .iter()
            .map(|c| {
                (
                    c.printing_id.as_str(),
                    c.variant.as_str(),
                    c.finish.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("C1-std", "Standard", "Normal"),
                ("C1-foil", "Hyperspace", "Foil")
            ]
        );
    }

    #[test]
    fn a_card_in_main_and_sideboard_shows_the_same_copies_on_both_lines() {
        let (_d, _s, _u, conn, a, _b) = stocked(4);
        let both = [
            line("C1", 2),
            Line {
                card_id: "C1".into(),
                zone: Zone::Sideboard,
                qty: 2,
                pick: Pick::default(),
            },
        ];
        assert!(matches!(
            set_lines(&conn, a, &both).unwrap(),
            SetLines::Set { .. }
        ));
        let states = line_states(&conn, a).unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].copies.len(), 4);
        for s in &states {
            assert_eq!(s.reserved as usize, s.copies.len());
            assert_eq!(
                s.copies.iter().map(|c| c.copy_id).collect::<Vec<_>>(),
                states[0]
                    .copies
                    .iter()
                    .map(|c| c.copy_id)
                    .collect::<Vec<_>>()
            );
        }
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
