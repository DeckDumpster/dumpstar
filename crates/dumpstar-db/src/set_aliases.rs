//! External set-name aliases: an import platform's set label mapped to a
//! catalog `set_code`. Collectr names sets differently from the catalog
//! ("SWU Promo" vs "Star Wars: Unlimited Promos"), so
//! the resolver ([`crate::import::resolve_set`]) consults this table as a
//! fallback after its code/ptcgo/name lookups.
//!
//! The registry is data: `data/set_aliases.json` is the canonical authoring
//! source, seeded into the `set_aliases` table at `dumpstar setup` time by
//! [`reconcile`]. New aliases are added by editing the JSON — no code change.

use rusqlite::{Connection, params};

use crate::error::Result;

/// `data/set_aliases.json` — the canonical alias registry.
pub(crate) const SET_ALIASES_SEED: &str = include_str!("../../../data/set_aliases.json");

/// One alias row. `note` is authoring documentation only — not stored.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SetAlias {
    pub alias: String,
    pub set_code: String,
}

/// Re-seed `set_aliases` from `data/set_aliases.json`. Called from
/// `open_shared`, so every read-write open of the catalog converges it before
/// the importer consults the table.
///
/// Each row is inserted only when its target `set_code` exists in this
/// catalog — a seed pointing at a set the current DB doesn't carry (e.g. a
/// minimal test catalog) is skipped rather than tripping the FK. Idempotent.
/// Returns the number of aliases actually written.
pub fn reconcile(conn: &mut Connection) -> Result<usize> {
    reconcile_from(conn, SET_ALIASES_SEED)
}

/// [`reconcile`] over an explicit JSON document, so a test can carry its own
/// seed rather than depend on whatever the shipped file holds today.
pub(crate) fn reconcile_from(conn: &mut Connection, json: &str) -> Result<usize> {
    let seed: Vec<SetAlias> = serde_json::from_str(json)?;
    let tx = conn.transaction()?;
    let mut written = 0usize;
    for a in &seed {
        let n = tx.execute(
            "INSERT INTO set_aliases (alias, set_code) \
             SELECT ?1, ?2 WHERE EXISTS (SELECT 1 FROM sets WHERE set_code = ?2) \
             ON CONFLICT(alias) DO UPDATE SET set_code = excluded.set_code",
            params![a.alias, a.set_code],
        )?;
        written += n;
    }
    tx.commit()?;
    Ok(written)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::open_shared;

    /// A test-owned seed: the shipped file starts empty for Star Wars: Unlimited.
    pub(crate) const TEST_SEED: &str = r#"[
      {"alias": "SWU Promo", "set_code": "swp"},
      {"alias": "OP Promos", "set_code": "opp"}
    ]"#;

    #[test]
    fn reconcile_seeds_aliases_for_present_sets_only() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        let mut conn = open_shared(&shared).unwrap();
        // Fresh catalog carries no sets → the seed rows are all skipped
        // (their FK targets are absent) rather than erroring.
        let n = reconcile_from(&mut conn, TEST_SEED).unwrap();
        assert_eq!(n, 0);

        // Add the two promo sets, then reconcile writes both aliases.
        conn.execute(
            "INSERT INTO sets (set_code, name, series) VALUES \
               ('swp', 'Star Wars: Unlimited Promos', 'SWU'), \
               ('opp', 'Organized Play Promos', 'SWU')",
            [],
        )
        .unwrap();
        let n = reconcile_from(&mut conn, TEST_SEED).unwrap();
        assert_eq!(n, 2);

        // Case-insensitive lookup lands the canonical code.
        let code: String = conn
            .query_row(
                "SELECT set_code FROM set_aliases WHERE alias = 'swu promo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(code, "swp");
    }
}
