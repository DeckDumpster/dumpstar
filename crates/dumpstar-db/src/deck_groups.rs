//! Repository for the `deck_groups` table — a named container that a set of
//! decks is managed through as one unit.
//!
//! A deck belongs to at most one group, and `decks.deck_group` holds that
//! group's name: the link is the name, so the MCP tools that take a group by
//! name, the `deck_group:` search term and the free-text column older builds
//! wrote all keep working. Every write that puts a name on a deck makes the
//! group's row ([`ensure`]); [`converge`] does the same for every name already
//! on a deck, which is the migration of the old free-text field.

use rusqlite::{Connection, OptionalExtension, params};

use crate::decks::{self, Deck};
use crate::error::{DbError, Result};
use crate::natsort::natural_cmp;

const COLS: &str = "g.id, g.name, g.description, g.format, g.created_at, g.updated_at, \
     (SELECT count(*) FROM decks d WHERE d.deck_group = g.name), \
     (SELECT count(*) FROM collection c JOIN decks d ON d.id = c.deck_id \
       WHERE d.deck_group = g.name)";

/// A deck group with its totals.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct DeckGroup {
    #[ts(type = "number")]
    pub id: i64,
    pub name: String,
    pub description: Option<String>,
    pub format: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// Decks in the group.
    #[ts(type = "number")]
    pub deck_count: i64,
    /// Cards reserved by the group's decks.
    #[ts(type = "number")]
    pub card_count: i64,
}

/// Fields for creating a group.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct NewDeckGroup {
    pub name: String,
    pub description: Option<String>,
    pub format: Option<String>,
}

/// Editable group fields. A `None` field is left unchanged; an empty
/// description or format clears it.
#[derive(Debug, Clone, Default, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct DeckGroupEdit {
    pub name: Option<String>,
    pub description: Option<String>,
    pub format: Option<String>,
}

/// What deleting a group does with its decks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteMode {
    /// The decks stay, without a group.
    Ungroup,
    /// The decks are deleted with the group; their cards return to the collection.
    DeleteDecks,
}

impl DeleteMode {
    /// The mode named by `"ungroup"` or `"delete"`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ungroup" => Some(Self::Ungroup),
            "delete" => Some(Self::DeleteDecks),
            _ => None,
        }
    }
}

/// What a group delete did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Deleted {
    /// Decks that were in the group.
    pub decks: u32,
    /// Of those, decks deleted with it (zero when ungrouped).
    pub decks_deleted: u32,
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<DeckGroup> {
    Ok(DeckGroup {
        id: r.get(0)?,
        name: r.get(1)?,
        description: r.get(2)?,
        format: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        deck_count: r.get(6)?,
        card_count: r.get(7)?,
    })
}

/// Make a group row of every name a deck carries, and drop the empty-string
/// groups older writers could leave. Idempotent; run on every open.
pub fn converge(conn: &Connection) -> Result<()> {
    conn.execute(
        "UPDATE decks SET deck_group = NULL WHERE deck_group = ''",
        [],
    )?;
    let now = crate::clock::now_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO deck_groups (name, created_at, updated_at) \
         SELECT DISTINCT deck_group, ?1, ?1 FROM decks WHERE deck_group IS NOT NULL",
        [now],
    )?;
    Ok(())
}

/// Make the group named `name` if it does not exist; `None` does nothing.
pub fn ensure(conn: &Connection, name: Option<&str>) -> Result<()> {
    let Some(name) = name.filter(|n| !n.is_empty()) else {
        return Ok(());
    };
    let now = crate::clock::now_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO deck_groups (name, created_at, updated_at) VALUES (?1, ?2, ?2)",
        params![name, now],
    )?;
    Ok(())
}

/// Create a group; returns its id. A name already in use is a conflict.
pub fn create(conn: &Connection, new: &NewDeckGroup) -> Result<i64> {
    if new.name.trim().is_empty() {
        return Err(DbError::Invalid("a group needs a name".into()));
    }
    if find_by_name(conn, &new.name)?.is_some() {
        return Err(DbError::Conflict(format!(
            "a group named {:?} already exists",
            new.name
        )));
    }
    let now = crate::clock::now_rfc3339();
    conn.execute(
        "INSERT INTO deck_groups (name, description, format, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![
            new.name,
            new.description.as_deref().filter(|s| !s.is_empty()),
            new.format.as_deref().filter(|s| !s.is_empty()),
            now
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Fetch one group.
pub fn get(conn: &Connection, id: i64) -> Result<Option<DeckGroup>> {
    Ok(conn
        .prepare(&format!("SELECT {COLS} FROM deck_groups g WHERE g.id = ?1"))?
        .query_row([id], from_row)
        .optional()?)
}

/// Fetch one group by exact name.
pub fn find_by_name(conn: &Connection, name: &str) -> Result<Option<DeckGroup>> {
    Ok(conn
        .prepare(&format!(
            "SELECT {COLS} FROM deck_groups g WHERE g.name = ?1"
        ))?
        .query_row([name], from_row)
        .optional()?)
}

/// List groups in natural name order, ties by id.
pub fn list(conn: &Connection) -> Result<Vec<DeckGroup>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLS} FROM deck_groups g ORDER BY g.id"))?;
    let mut groups: Vec<DeckGroup> = stmt
        .query_map([], from_row)?
        .collect::<rusqlite::Result<_>>()?;
    groups.sort_by(|a, b| natural_cmp(&a.name, &b.name).then(a.id.cmp(&b.id)));
    Ok(groups)
}

/// The decks in a group, in natural name order.
pub fn decks_of(conn: &Connection, id: i64) -> Result<Vec<Deck>> {
    let group = get(conn, id)?.ok_or_else(|| DbError::NotFound(format!("deck group {id}")))?;
    Ok(decks::list(conn)?
        .into_iter()
        .filter(|d| d.group.as_deref() == Some(group.name.as_str()))
        .collect())
}

/// Edit a group. A rename moves its decks with it, in one transaction.
/// Returns whether the group exists.
pub fn update(conn: &Connection, id: i64, edit: &DeckGroupEdit) -> Result<bool> {
    decks::immediate(conn, |conn| {
        let Some(old) = get(conn, id)? else {
            return Ok(false);
        };
        let now = crate::clock::now_rfc3339();
        if let Some(name) = edit.name.as_deref().filter(|n| *n != old.name) {
            if name.trim().is_empty() {
                return Err(DbError::Invalid("a group needs a name".into()));
            }
            if find_by_name(conn, name)?.is_some() {
                return Err(DbError::Conflict(format!(
                    "a group named {name:?} already exists"
                )));
            }
            conn.execute(
                "UPDATE decks SET deck_group = ?1, updated_at = ?2 WHERE deck_group = ?3",
                params![name, now, old.name],
            )?;
            conn.execute(
                "UPDATE deck_groups SET name = ?1 WHERE id = ?2",
                params![name, id],
            )?;
        }
        let clearable = |v: &Option<String>, current: Option<String>| match v {
            None => current,
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s.clone()),
        };
        conn.execute(
            "UPDATE deck_groups SET description = ?1, format = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                clearable(&edit.description, old.description),
                clearable(&edit.format, old.format),
                now,
                id
            ],
        )?;
        Ok(true)
    })
}

/// Free every copy held by the group's decks; the decks and their lines stay.
/// Returns the number of copies released.
pub fn release_all(conn: &Connection, id: i64) -> Result<u32> {
    decks::immediate(conn, |conn| {
        let ids = member_ids(conn, id)?;
        let mut released = 0;
        for deck in ids {
            released += decks::release_in(conn, deck)?;
        }
        Ok(released)
    })
}

/// Delete a group. Its decks are ungrouped or deleted with it, per `mode`.
pub fn delete(conn: &Connection, id: i64, mode: DeleteMode) -> Result<Deleted> {
    decks::immediate(conn, |conn| {
        let ids = member_ids(conn, id)?;
        let name: String =
            conn.query_row("SELECT name FROM deck_groups WHERE id = ?1", [id], |r| {
                r.get(0)
            })?;
        let mut decks_deleted = 0;
        match mode {
            DeleteMode::Ungroup => {
                conn.execute(
                    "UPDATE decks SET deck_group = NULL, updated_at = ?1 WHERE deck_group = ?2",
                    params![crate::clock::now_rfc3339(), name],
                )?;
            }
            DeleteMode::DeleteDecks => {
                for deck in &ids {
                    if decks::delete_in(conn, *deck)? {
                        decks_deleted += 1;
                    }
                }
            }
        }
        conn.execute("DELETE FROM deck_groups WHERE id = ?1", [id])?;
        Ok(Deleted {
            decks: ids.len() as u32,
            decks_deleted,
        })
    })
}

fn member_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let name: String = conn
        .query_row("SELECT name FROM deck_groups WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .optional()?
        .ok_or_else(|| DbError::NotFound(format!("deck group {id}")))?;
    let mut stmt = conn.prepare("SELECT id FROM decks WHERE deck_group = ?1 ORDER BY id")?;
    Ok(stmt
        .query_map([name], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decks::{NewDeck, create as new_deck};
    use crate::{connect_user, open_shared};

    fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        open_shared(&shared).unwrap();
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    fn deck(conn: &Connection, name: &str, group: Option<&str>) -> i64 {
        new_deck(
            conn,
            &NewDeck {
                name: name.into(),
                group: group.map(String::from),
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn existing_group_strings_become_groups_with_their_decks() {
        let (dir, conn) = conn();
        // A deck group as the old free-text field left it: no group row.
        for (name, group) in [
            ("P1", "Cube"),
            ("P2", "Cube"),
            ("Solo", "Other"),
            ("Free", ""),
        ] {
            conn.execute(
                "INSERT INTO decks (name, deck_group, created_at, updated_at) VALUES (?1, ?2, 'x', 'x')",
                params![name, group],
            )
            .unwrap();
        }
        conn.execute("DELETE FROM deck_groups", []).unwrap();
        drop(conn);
        // Reopening converges, as a deploy does.
        let shared = dir.path().join("shared.sqlite");
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let groups = list(&conn).unwrap();
        let summary: Vec<_> = groups
            .iter()
            .map(|g| (g.name.as_str(), g.deck_count))
            .collect();
        assert_eq!(summary, [("Cube", 2), ("Other", 1)]);
        let cube = find_by_name(&conn, "Cube").unwrap().unwrap();
        let members: Vec<_> = decks_of(&conn, cube.id)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        assert_eq!(members, ["P1", "P2"]);
        let free: Option<String> = conn
            .query_row(
                "SELECT deck_group FROM decks WHERE name = 'Free'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(free, None);
    }

    #[test]
    fn crudl_with_rename_carrying_the_decks() {
        let (_d, conn) = conn();
        let id = create(
            &conn,
            &NewDeckGroup {
                name: "Cube".into(),
                description: Some("d".into()),
                format: Some("Jumpstart".into()),
            },
        )
        .unwrap();
        assert!(matches!(
            create(
                &conn,
                &NewDeckGroup {
                    name: "Cube".into(),
                    ..Default::default()
                }
            ),
            Err(DbError::Conflict(_))
        ));
        let a = deck(&conn, "Pack 1", Some("Cube"));
        assert_eq!(get(&conn, id).unwrap().unwrap().deck_count, 1);
        assert!(
            update(
                &conn,
                id,
                &DeckGroupEdit {
                    name: Some("Cube 2".into()),
                    description: Some(String::new()),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        let g = get(&conn, id).unwrap().unwrap();
        assert_eq!(
            (
                g.name.as_str(),
                g.description,
                g.format.as_deref(),
                g.deck_count
            ),
            ("Cube 2", None, Some("Jumpstart"), 1)
        );
        assert_eq!(
            decks::get(&conn, a).unwrap().unwrap().group.as_deref(),
            Some("Cube 2")
        );
        let other = create(
            &conn,
            &NewDeckGroup {
                name: "B".into(),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(
            update(
                &conn,
                other,
                &DeckGroupEdit {
                    name: Some("Cube 2".into()),
                    ..Default::default()
                }
            ),
            Err(DbError::Conflict(_))
        ));
        assert!(!update(&conn, 999, &DeckGroupEdit::default()).unwrap());
        // An empty group on a deck takes it out of its group; the group stays.
        decks::update(
            &conn,
            a,
            &decks::DeckEdit {
                group: Some(String::new()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(decks::get(&conn, a).unwrap().unwrap().group, None);
        assert_eq!(get(&conn, id).unwrap().unwrap().deck_count, 0);
        assert_eq!(list(&conn).unwrap().len(), 2);
    }

    #[test]
    fn naming_an_unknown_group_on_a_deck_creates_it() {
        let (_d, conn) = conn();
        deck(&conn, "P", Some("Fresh"));
        assert!(find_by_name(&conn, "Fresh").unwrap().is_some());
    }

    #[test]
    fn delete_ungroups_or_takes_the_decks_with_it() {
        let (_d, conn) = conn();
        for (mode, kept) in [
            (DeleteMode::Ungroup, true),
            (DeleteMode::DeleteDecks, false),
        ] {
            let id = create(
                &conn,
                &NewDeckGroup {
                    name: format!("{mode:?}"),
                    ..Default::default()
                },
            )
            .unwrap();
            let d = deck(&conn, "P", Some(&format!("{mode:?}")));
            let done = delete(&conn, id, mode).unwrap();
            assert_eq!(done.decks, 1);
            assert_eq!(done.decks_deleted, if kept { 0 } else { 1 });
            let found = decks::get(&conn, d).unwrap();
            assert_eq!(found.is_some(), kept);
            if let Some(f) = found {
                assert_eq!(f.group, None);
            }
            assert!(get(&conn, id).unwrap().is_none());
        }
        assert!(matches!(
            delete(&conn, 999, DeleteMode::Ungroup),
            Err(DbError::NotFound(_))
        ));
    }
}
