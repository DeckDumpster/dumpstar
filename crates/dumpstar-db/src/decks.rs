//! Repository for the `decks` table — physical decks built from the
//! collection. Decks carry a 3-state lifecycle: idea → ready → built.

use std::collections::HashMap;

use dumpstar_core::deck::{DeckEntry, OwnedLine, Zone};
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::Result;

const COLS: &str = "d.id, d.name, d.description, d.format, d.owner, d.state, \
     d.sleeve_color, d.storage_location, d.notes, d.created_at, d.updated_at, \
     (SELECT count(*) FROM collection c WHERE c.deck_id = d.id)";

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
    })
}

/// Create a deck; returns its id.
pub fn create(conn: &Connection, new: &NewDeck) -> Result<i64> {
    let now = crate::clock::now_rfc3339();
    conn.execute(
        "INSERT INTO decks \
           (name, description, format, owner, state, sleeve_color, \
            storage_location, notes, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
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

/// List decks, alphabetically.
pub fn list(conn: &Connection) -> Result<Vec<Deck>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM decks d ORDER BY d.name"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Update editable fields. Returns whether a row changed.
pub fn update(conn: &Connection, id: i64, edit: &DeckEdit) -> Result<bool> {
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
           updated_at       = ?10 \
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

/// Delete a deck. Its cards are un-assigned (collection.deck_id is
/// `ON DELETE SET NULL`), never deleted.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
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
}
