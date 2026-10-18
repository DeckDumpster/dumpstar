//! Application-layer foreign-key checks against the shared catalog.
//!
//! SQLite cannot enforce `REFERENCES` across an `ATTACH`ed database, so
//! repository code validates catalog keys here before writing user rows
//! (PLAN.md §3.5). These helpers query the catalog tables unqualified — they
//! work both on a direct shared connection and on a user connection with the
//! shared catalog attached (see [`crate::attach_shared_readonly`]).

use rusqlite::Connection;

use crate::error::Result;

/// Whether a card with this id exists in the catalog.
pub fn card_exists(conn: &Connection, card_id: &str) -> Result<bool> {
    Ok(conn
        .prepare("SELECT 1 FROM cards WHERE card_id = ?1")?
        .exists([card_id])?)
}

/// Whether a printing with this id exists — either in the shared
/// catalog or in the user-DB `user_printings` table (the "Missing
/// Variant" escape hatch, decision dumpstar-x7k). Both count as
/// valid FK targets for `collection.printing_id` and friends.
pub fn printing_exists(conn: &Connection, printing_id: &str) -> Result<bool> {
    Ok(conn
        .prepare(
            "SELECT 1 FROM printings WHERE printing_id = ?1 \
             UNION ALL \
             SELECT 1 FROM user_printings WHERE printing_id = ?1",
        )?
        .exists([printing_id])?)
}

/// Whether a sealed product with this id exists in the catalog.
pub fn sealed_product_exists(conn: &Connection, product_id: i64) -> Result<bool> {
    Ok(conn
        .prepare("SELECT 1 FROM sealed_products WHERE product_id = ?1")?
        .exists([product_id])?)
}

/// Every printing the application can show, as one relation: the catalog's
/// `printings` and the tenant's `user_printings` (the "Missing Variant"
/// escape hatch, decision dumpstar-x7k).
///
/// Columns: `printing_id, card_id, set_code, number, variant, finish, rarity,
/// artist, image_url, variant_description, back_image_url`. A tenant-invented printing has no
/// set or number of its own; it borrows the card's lead printing (the
/// Standard one when there is one), so it files beside the card it varies.
/// `variant_description` is non-NULL only for those. Alias it `p`. The macro
/// form exists so it can sit inside `concat!`.
#[macro_export]
macro_rules! printing_rel {
    () => {
        "( \
     SELECT printing_id, card_id, set_code, number, variant, finish, rarity, artist, \
            image_url, NULL AS variant_description, back_image_url \
       FROM printings \
     UNION ALL \
     SELECT up.printing_id, up.card_id, \
            (SELECT x.set_code FROM printings x WHERE x.card_id = up.card_id \
              ORDER BY x.variant <> 'Standard', x.printing_id LIMIT 1), \
            (SELECT x.number FROM printings x WHERE x.card_id = up.card_id \
              ORDER BY x.variant <> 'Standard', x.printing_id LIMIT 1), \
            up.variant, 'Normal', NULL, NULL, NULL, up.description, NULL \
       FROM user_printings up \
     )"
    };
}

/// [`printing_rel!`] as a value, for code that formats SQL.
pub const PRINTING_REL: &str = crate::printing_rel!();

/// The id of a card's lead printing — the one that stands for the card where a
/// single printing must be named (a wishlist row, a deck row): the Standard
/// variant first, its Normal finish first, then the lowest id. `$card` is a SQL
/// expression naming a `card_id`.
#[macro_export]
macro_rules! lead_printing_id {
    ($card:expr) => {
        concat!(
            "(SELECT x.printing_id FROM printings x WHERE x.card_id = ",
            $card,
            " ORDER BY x.variant <> 'Standard', x.finish <> 'Normal', x.printing_id LIMIT 1)"
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use crate::{connect_user, open_shared};

    /// A tenant connection over a catalog holding:
    ///
    /// * `LUKE` — a Hyperspace Normal (`h-n`, SOR 261), a Standard Foil
    ///   (`s-f`, SOR 5) and a Standard Normal (`s-n`, SOR 5);
    /// * `VADER` — Hyperspace printings only (`v-h-f` Foil and `v-h-n` Normal,
    ///   SHD 300);
    /// * `GHOST` — a card with no printing at all.
    fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::set(&c, "SHD", "Shadows of the Galaxy");
            for (id, name) in [("LUKE", "Luke"), ("VADER", "Vader"), ("GHOST", "Ghost")] {
                testkit::card(&c, id, name);
            }
            testkit::printing(&c, "h-n", "LUKE", "SOR", "261", "Hyperspace", "Normal");
            testkit::printing(&c, "s-f", "LUKE", "SOR", "5", "Standard", "Foil");
            testkit::printing(&c, "s-n", "LUKE", "SOR", "5", "Standard", "Normal");
            testkit::printing(&c, "v-h-f", "VADER", "SHD", "300", "Hyperspace", "Foil");
            testkit::printing(&c, "v-h-n", "VADER", "SHD", "300", "Hyperspace", "Normal");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        for (id, card, desc) in [
            ("u-luke", "LUKE", "stamped"),
            ("u-vader", "VADER", "misprint"),
            ("u-ghost", "GHOST", "orphan"),
        ] {
            conn.execute(
                "INSERT INTO user_printings (printing_id, card_id, variant, description) \
                 VALUES (?1, ?2, 'invented', ?3)",
                rusqlite::params![id, card, desc],
            )
            .unwrap();
        }
        (dir, conn)
    }

    type Row = (
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
        Option<String>,
    );

    fn rel_rows(conn: &Connection) -> Vec<Row> {
        let sql = format!(
            "SELECT p.printing_id, p.card_id, p.set_code, p.number, p.variant, p.finish, \
                    p.variant_description FROM {} p ORDER BY p.printing_id",
            crate::printing_rel!()
        );
        conn.prepare(&sql)
            .unwrap()
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn row<'a>(rows: &'a [Row], id: &str) -> &'a Row {
        rows.iter()
            .find(|r| r.0 == id)
            .unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn printing_rel_unions_catalog_and_user_printings() {
        let (_d, conn) = conn();
        let rows = rel_rows(&conn);
        assert_eq!(rows.len(), 8, "5 catalog printings + 3 user printings");
        // The constant is the same text.
        assert_eq!(PRINTING_REL, crate::printing_rel!());

        // A catalog printing is itself, with no variant description.
        let s_f = row(&rows, "s-f");
        assert_eq!(s_f.2.as_deref(), Some("SOR"));
        assert_eq!(s_f.3.as_deref(), Some("5"));
        assert_eq!((s_f.4.as_str(), s_f.5.as_str()), ("Standard", "Foil"));
        assert_eq!(s_f.6, None);
    }

    #[test]
    fn a_user_printing_borrows_the_lead_printings_set_and_number() {
        let (_d, conn) = conn();
        let rows = rel_rows(&conn);

        // LUKE has a Standard printing: the invented one files beside it
        // (SOR 5), not beside the Hyperspace (SOR 261) that sorts first by id.
        let u = row(&rows, "u-luke");
        assert_eq!(u.1, "LUKE");
        assert_eq!(u.2.as_deref(), Some("SOR"));
        assert_eq!(u.3.as_deref(), Some("5"));
        assert_eq!((u.4.as_str(), u.5.as_str()), ("invented", "Normal"));
        assert_eq!(u.6.as_deref(), Some("stamped"));

        // VADER has no Standard one: it borrows from the lowest id.
        let u = row(&rows, "u-vader");
        assert_eq!(u.2.as_deref(), Some("SHD"));
        assert_eq!(u.3.as_deref(), Some("300"));

        // A card with no catalog printing has nowhere to file: NULL, not a
        // dropped row.
        let u = row(&rows, "u-ghost");
        assert_eq!((u.2.clone(), u.3.clone()), (None, None));
        assert_eq!(u.6.as_deref(), Some("orphan"));
    }

    #[test]
    fn lead_printing_id_prefers_standard_then_normal_then_lowest_id() {
        let (_d, conn) = conn();
        let lead = |card: &str| -> Option<String> {
            let sql = format!(
                "SELECT {} FROM (SELECT ?1 AS card_id) q",
                crate::lead_printing_id!("q.card_id")
            );
            conn.query_row(&sql, [card], |r| r.get(0)).unwrap()
        };
        // Standard beats the Hyperspace that sorts first by id, and Normal
        // beats the Foil that does too.
        assert_eq!(lead("LUKE").as_deref(), Some("s-n"));
        // No Standard: Hyperspace, its Normal finish first.
        assert_eq!(lead("VADER").as_deref(), Some("v-h-n"));
        assert_eq!(lead("GHOST"), None);
    }
}
