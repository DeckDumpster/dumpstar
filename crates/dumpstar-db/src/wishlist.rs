//! Repository for the wishlist — cards the user wants to acquire.

use rusqlite::{Connection, params};

use crate::catalog;
use crate::error::{DbError, Result};

const COLS: &str = "w.id, w.card_id, w.printing_id, w.max_price, w.priority, \
     w.notes, w.added_at, w.source, w.fulfilled_at, \
     w.min_condition, w.quantity, w.quantity_have, \
     cd.set_code, s.name AS set_name, cd.number, cd.name, cd.rarity, cd.image_small";

/// A wishlist entry joined to its card.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct WishlistEntry {
    #[ts(type = "number")]
    pub id: i64,
    pub card_id: String,
    /// A specific wanted printing, or null for "any printing".
    pub printing_id: Option<String>,
    pub max_price: Option<f64>,
    #[ts(type = "number")]
    pub priority: i64,
    pub notes: Option<String>,
    pub added_at: String,
    pub source: String,
    pub fulfilled_at: Option<String>,
    /// Worst acceptable condition, or null for any.
    pub min_condition: Option<String>,
    #[ts(type = "number")]
    pub quantity: i64,
    /// Matching copies added so far; the want is fulfilled at `quantity`.
    #[ts(type = "number")]
    pub quantity_have: i64,
    pub set_code: String,
    pub set_name: String,
    pub number: String,
    pub name: String,
    pub rarity: Option<String>,
    pub image_small: Option<String>,
}

/// Fields for adding a wish.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewWish {
    pub card_id: String,
    pub printing_id: Option<String>,
    pub max_price: Option<f64>,
    #[ts(type = "number | null")]
    pub priority: Option<i64>,
    pub notes: Option<String>,
    pub min_condition: Option<String>,
    #[ts(type = "number | null")]
    pub quantity: Option<i64>,
}

/// Editable wish fields. A `None` field is left unchanged.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct WishEdit {
    pub max_price: Option<f64>,
    #[ts(type = "number | null")]
    pub priority: Option<i64>,
    pub notes: Option<String>,
    pub min_condition: Option<String>,
    #[ts(type = "number | null")]
    pub quantity: Option<i64>,
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<WishlistEntry> {
    Ok(WishlistEntry {
        id: r.get(0)?,
        card_id: r.get(1)?,
        printing_id: r.get(2)?,
        max_price: r.get(3)?,
        priority: r.get(4)?,
        notes: r.get(5)?,
        added_at: r.get(6)?,
        source: r.get(7)?,
        fulfilled_at: r.get(8)?,
        min_condition: r.get(9)?,
        quantity: r.get(10)?,
        quantity_have: r.get(11)?,
        set_code: r.get(12)?,
        set_name: r.get(13)?,
        number: r.get(14)?,
        name: r.get(15)?,
        rarity: r.get(16)?,
        image_small: r.get(17)?,
    })
}

/// Add a wish. Validates the card against the catalog. Returns the new id.
pub fn add(conn: &Connection, new: &NewWish) -> Result<i64> {
    if !catalog::card_exists(conn, &new.card_id)? {
        return Err(DbError::NotFound(format!(
            "card '{}' is not in the catalog",
            new.card_id
        )));
    }
    if let Some(pid) = &new.printing_id {
        let owner = printing_card(conn, pid)?;
        if owner.as_deref() != Some(new.card_id.as_str()) {
            return Err(DbError::NotFound(format!(
                "printing '{pid}' is not a printing of card '{}'",
                new.card_id
            )));
        }
    }
    check_condition(conn, new.min_condition.as_deref())?;
    let quantity = new.quantity.unwrap_or(1);
    check_quantity(quantity)?;
    conn.execute(
        "INSERT INTO wishlist \
           (card_id, printing_id, max_price, priority, notes, added_at, source, \
            min_condition, quantity) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'manual', ?7, ?8)",
        params![
            new.card_id,
            new.printing_id,
            new.max_price,
            new.priority.unwrap_or(0),
            new.notes,
            crate::clock::now_rfc3339(),
            new.min_condition,
            quantity,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// List wishlist entries. `include_fulfilled` false returns only active wishes.
/// Ordered by priority (highest first), then most-recently added.
pub fn list(conn: &Connection, include_fulfilled: bool) -> Result<Vec<WishlistEntry>> {
    let filter = if include_fulfilled {
        ""
    } else {
        "WHERE w.fulfilled_at IS NULL "
    };
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM wishlist w \
           JOIN cards cd ON w.card_id = cd.card_id \
           JOIN sets s ON cd.set_code = s.set_code \
         {filter}ORDER BY w.priority DESC, w.id DESC"
    ))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Update a wish's editable fields. Returns whether a row changed.
pub fn update(conn: &Connection, id: i64, edit: &WishEdit) -> Result<bool> {
    check_condition(conn, edit.min_condition.as_deref())?;
    if let Some(q) = edit.quantity {
        check_quantity(q)?;
    }
    let n = conn.execute(
        "UPDATE wishlist SET \
           max_price     = COALESCE(?2, max_price), \
           priority      = COALESCE(?3, priority), \
           notes         = COALESCE(?4, notes), \
           min_condition = COALESCE(?5, min_condition), \
           quantity      = COALESCE(?6, quantity) \
         WHERE id = ?1",
        params![
            id,
            edit.max_price,
            edit.priority,
            edit.notes,
            edit.min_condition,
            edit.quantity
        ],
    )?;
    Ok(n > 0)
}

/// Mark a wish fulfilled (or clear it). Returns whether a row changed.
pub fn set_fulfilled(conn: &Connection, id: i64, fulfilled: bool) -> Result<bool> {
    let stamp = fulfilled.then(crate::clock::now_rfc3339);
    let n = conn.execute(
        "UPDATE wishlist SET fulfilled_at = ?2 WHERE id = ?1",
        params![id, stamp],
    )?;
    Ok(n > 0)
}

/// Delete a wish.
pub fn delete(conn: &Connection, id: i64) -> Result<bool> {
    Ok(conn.execute("DELETE FROM wishlist WHERE id = ?1", [id])? > 0)
}

fn check_quantity(q: i64) -> Result<()> {
    if q < 1 {
        return Err(DbError::Invalid(
            "a want's quantity must be at least 1".into(),
        ));
    }
    Ok(())
}

fn check_condition(conn: &Connection, name: Option<&str>) -> Result<()> {
    let Some(name) = name else { return Ok(()) };
    if !conn
        .prepare("SELECT 1 FROM conditions WHERE name = ?1")?
        .exists([name])?
    {
        return Err(DbError::Invalid(format!("unknown condition '{name}'")));
    }
    Ok(())
}

/// The card a printing belongs to, from the catalog or `user_printings`.
fn printing_card(conn: &Connection, printing_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT card_id FROM printings WHERE printing_id = ?1 \
             UNION ALL \
             SELECT card_id FROM user_printings WHERE printing_id = ?1",
            [printing_id],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?)
}

/// Credit a newly added copy to the active want it satisfies, if any.
///
/// A want matches when it names the copy's card and either no printing or
/// exactly this printing, the copy's condition is at least the want's
/// `min_condition`, and the copy's purchase price (when recorded) is within
/// `max_price`. A copy counts toward one want only — the highest-priority,
/// oldest — and a want reaching its `quantity` is marked fulfilled.
/// Returns the id of the want credited.
pub fn credit_copy(
    conn: &Connection,
    printing_id: &str,
    condition: &str,
    purchase_price: Option<f64>,
) -> Result<Option<i64>> {
    let Some(card_id) = printing_card(conn, printing_id)? else {
        return Ok(None);
    };
    let id: Option<i64> = conn
        .query_row(
            "SELECT w.id FROM wishlist w \
               LEFT JOIN conditions wc ON wc.name = w.min_condition \
               LEFT JOIN conditions cc ON cc.name = ?3 \
              WHERE w.fulfilled_at IS NULL AND w.card_id = ?1 \
                AND (w.printing_id IS NULL OR w.printing_id = ?2) \
                AND (w.min_condition IS NULL OR (cc.rank IS NOT NULL AND cc.rank <= wc.rank)) \
                AND (w.max_price IS NULL OR ?4 IS NULL OR ?4 <= w.max_price) \
              ORDER BY w.priority DESC, w.id ASC LIMIT 1",
            params![card_id, printing_id, condition, purchase_price],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    let Some(id) = id else { return Ok(None) };
    conn.execute(
        "UPDATE wishlist SET quantity_have = quantity_have + 1, \
           fulfilled_at = CASE WHEN quantity_have + 1 >= quantity THEN ?2 ELSE NULL END \
         WHERE id = ?1",
        params![id, crate::clock::now_rfc3339()],
    )?;
    Ok(Some(id))
}

/// Whether the card already has an (active) wish — used to avoid duplicates.
pub fn exists_for_card(conn: &Connection, card_id: &str) -> Result<bool> {
    Ok(conn
        .prepare("SELECT 1 FROM wishlist WHERE card_id = ?1 AND fulfilled_at IS NULL")?
        .exists([card_id])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_user, open_shared};

    fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            c.execute(
                "INSERT INTO sets (set_code, name, series) VALUES ('sor', 'Spark of Rebellion', 'SWU')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO cards (card_id, set_code, number, number_sortable, name) \
                 VALUES ('sor-6', 'sor', '6', 6, 'Darth Vader')",
                [],
            )
            .unwrap();
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    #[test]
    fn add_list_fulfill_delete() {
        let (_d, conn) = conn();
        let id = add(
            &conn,
            &NewWish {
                card_id: "sor-6".into(),
                priority: Some(5),
                max_price: Some(200.0),
                ..Default::default()
            },
        )
        .unwrap();

        let active = list(&conn, false).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].name, "Darth Vader");
        assert_eq!(active[0].priority, 5);
        assert!(exists_for_card(&conn, "sor-6").unwrap());

        // Fulfilled wishes drop out of the active list.
        assert!(set_fulfilled(&conn, id, true).unwrap());
        assert_eq!(list(&conn, false).unwrap().len(), 0);
        assert_eq!(list(&conn, true).unwrap().len(), 1);
        assert!(!exists_for_card(&conn, "sor-6").unwrap());

        assert!(delete(&conn, id).unwrap());
        assert_eq!(list(&conn, true).unwrap().len(), 0);
    }

    fn with_printings() -> (tempfile::TempDir, Connection) {
        let (d, conn) = conn();
        let shared = d.path().join("shared.sqlite");
        let c = open_shared(&shared).unwrap();
        for v in ["normal", "foil"] {
            c.execute(
                "INSERT INTO printings (printing_id, card_id, variant) \
                 VALUES (?1, 'sor-6', ?2)",
                params![format!("sor-6-{v}"), v],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO cards (card_id, set_code, number, number_sortable, name) \
             VALUES ('sor-7', 'sor', '7', 7, 'Other')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO printings (printing_id, card_id, variant) \
             VALUES ('sor-7-normal', 'sor-7', 'normal')",
            [],
        )
        .unwrap();
        drop(c);
        (d, conn)
    }

    fn want(conn: &Connection, printing: Option<&str>) -> i64 {
        add(
            conn,
            &NewWish {
                card_id: "sor-6".into(),
                printing_id: printing.map(String::from),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn active(conn: &Connection) -> usize {
        list(conn, false).unwrap().len()
    }

    #[test]
    fn any_printing_want_is_fulfilled_by_any_matching_copy() {
        let (_d, mut conn) = with_printings();
        want(&conn, None);
        crate::collection::add(
            &mut conn,
            &crate::collection::NewCopy {
                printing_id: "sor-6-foil".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(active(&conn), 0);
    }

    #[test]
    fn specific_printing_want_is_not_fulfilled_by_a_different_printing() {
        let (_d, mut conn) = with_printings();
        want(&conn, Some("sor-6-foil"));
        let copy = |p: &str| crate::collection::NewCopy {
            printing_id: p.into(),
            source: "manual_id".into(),
            ..Default::default()
        };
        crate::collection::add(&mut conn, &copy("sor-6-normal")).unwrap();
        crate::collection::add(&mut conn, &copy("sor-7-normal")).unwrap();
        assert_eq!(active(&conn), 1);
        crate::collection::add(&mut conn, &copy("sor-6-foil")).unwrap();
        assert_eq!(active(&conn), 0);
    }

    #[test]
    fn condition_price_and_quantity_gate_fulfilment() {
        let (_d, mut conn) = with_printings();
        let id = add(
            &conn,
            &NewWish {
                card_id: "sor-6".into(),
                min_condition: Some("Lightly Played".into()),
                max_price: Some(10.0),
                quantity: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
        let copy = |cond: &str, price: f64| crate::collection::NewCopy {
            printing_id: "sor-6-normal".into(),
            condition: Some(cond.into()),
            purchase_price: Some(price),
            source: "manual_id".into(),
            ..Default::default()
        };
        // Too worn, too dear: neither counts.
        crate::collection::add(&mut conn, &copy("Heavily Played", 5.0)).unwrap();
        crate::collection::add(&mut conn, &copy("Near Mint", 50.0)).unwrap();
        assert_eq!(list(&conn, false).unwrap()[0].quantity_have, 0);
        // Two good ones: the first leaves it open, the second fulfils it.
        crate::collection::add(&mut conn, &copy("Near Mint", 5.0)).unwrap();
        assert_eq!(active(&conn), 1);
        crate::collection::add(&mut conn, &copy("Lightly Played", 10.0)).unwrap();
        assert_eq!(active(&conn), 0);
        let all = list(&conn, true).unwrap();
        assert_eq!((all[0].id, all[0].quantity_have), (id, 2));
    }

    #[test]
    fn add_rejects_foreign_printing_bad_condition_and_zero_quantity() {
        let (_d, conn) = with_printings();
        let bad = |w: NewWish| add(&conn, &w).unwrap_err();
        let base = || NewWish {
            card_id: "sor-6".into(),
            ..Default::default()
        };
        assert!(matches!(
            bad(NewWish {
                printing_id: Some("sor-7-normal".into()),
                ..base()
            }),
            DbError::NotFound(_)
        ));
        assert!(matches!(
            bad(NewWish {
                min_condition: Some("Mint-ish".into()),
                ..base()
            }),
            DbError::Invalid(_)
        ));
        assert!(matches!(
            bad(NewWish {
                quantity: Some(0),
                ..base()
            }),
            DbError::Invalid(_)
        ));
    }

    #[test]
    fn add_rejects_unknown_card() {
        let (_d, conn) = conn();
        let err = add(
            &conn,
            &NewWish {
                card_id: "nope-1".into(),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(matches!(err, DbError::NotFound(_)));
    }
}
