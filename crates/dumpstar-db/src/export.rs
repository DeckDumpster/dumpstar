//! CSV export (PLAN.md §9) — the collection rendered as a ManaBox-shaped
//! CSV that re-imports cleanly through `import::manabox`. A printing is named
//! by (set code, collector number, finish): the number carries the variant.

use rusqlite::Connection;

use crate::error::Result;

/// The ManaBox export header, in ManaBox's column order.
const HEADER: &[&str] = &[
    "Set code",
    "Set name",
    "Collector number",
    "Foil",
    "Rarity",
    "Quantity",
    "ManaBox ID",
    "Scryfall ID",
    "Purchase price",
    "Misprint",
    "Altered",
    "Condition",
    "Language",
    "Purchase price currency",
];

/// Export the whole collection as a ManaBox-shaped CSV string. One row per
/// physical copy (Quantity is always 1 — no aggregation), with the `Foil`
/// column holding `foil` or `normal`.
pub fn manabox_csv(conn: &Connection) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT s.set_code, s.name, p.number, p.finish, p.rarity, \
                c.purchase_price, c.condition, c.language, c.tags \
         FROM collection c \
           JOIN printings p ON c.printing_id = p.printing_id \
           JOIN sets s ON p.set_code = s.set_code \
         ORDER BY s.set_code, CAST(p.number AS INTEGER), p.number, p.finish, c.id",
    )?;

    let mut writer = csv::Writer::from_writer(Vec::new());
    writer.write_record(HEADER).map_err(csv_err)?;

    let mut rows = stmt.query([])?;
    while let Some(r) = rows.next()? {
        let set_code: String = r.get(0)?;
        let set_name: String = r.get(1)?;
        let number: String = r.get(2)?;
        let finish: String = r.get(3)?;
        let rarity: Option<String> = r.get(4)?;
        let purchase_price: Option<f64> = r.get(5)?;
        let condition: String = r.get(6)?;
        let language: String = r.get(7)?;
        let tags: Option<String> = r.get(8)?;

        let tags = tags.unwrap_or_default();
        let price = purchase_price
            .map(|p| format!("{p:.2}"))
            .unwrap_or_default();
        let currency = if purchase_price.is_some() { "USD" } else { "" };

        writer
            .write_record([
                &set_code,
                &set_name,
                &number,
                &String::from(if finish == "Foil" { "foil" } else { "normal" }),
                rarity.as_deref().unwrap_or(""),
                "1",
                "",
                "",
                &price,
                bool_str(tags.contains("misprint")),
                bool_str(tags.contains("altered")),
                &condition,
                &language,
                currency,
            ])
            .map_err(csv_err)?;
    }

    let bytes = writer
        .into_inner()
        .map_err(|e| crate::DbError::Import(format!("CSV write error: {e}")))?;
    String::from_utf8(bytes)
        .map_err(|e| crate::DbError::Import(format!("export produced invalid UTF-8: {e}")))
}

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// A `csv` writer error surfaced as a `DbError`.
fn csv_err(e: csv::Error) -> crate::DbError {
    crate::DbError::Import(format!("CSV write error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::{self, NewCopy};
    use crate::testkit;
    use crate::{connect_user, open_shared};

    #[test]
    fn exports_a_manabox_shaped_csv() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::card(&c, "SOR-5", "Luke Skywalker");
            testkit::printing(
                &c,
                "SOR-5-normal",
                "SOR-5",
                "SOR",
                "5",
                "Standard",
                "Normal",
            );
            testkit::printing(&c, "SOR-5-foil", "SOR-5", "SOR", "5", "Standard", "Foil");
            c.execute("UPDATE printings SET rarity = 'Legendary'", [])
                .unwrap();
        }
        let mut conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        collection::add(
            &mut conn,
            &NewCopy {
                printing_id: "SOR-5-normal".into(),
                source: "manual_id".into(),
                purchase_price: Some(12.5),
                ..Default::default()
            },
        )
        .unwrap();
        collection::add(
            &mut conn,
            &NewCopy {
                printing_id: "SOR-5-foil".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let csv = manabox_csv(&conn).unwrap();
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 3); // header + two copies
        assert!(lines[0].starts_with("Set code,Set name,Collector number,Foil"));
        // Rows of one number order by finish ('Foil' sorts before 'Normal').
        assert!(lines[1].starts_with("SOR,Spark of Rebellion,5,foil,Legendary"));
        assert!(lines[2].starts_with("SOR,Spark of Rebellion,5,normal,Legendary"));
        assert!(lines[2].contains("12.50"));
        assert!(lines[2].contains("USD"));

        // The export round-trips back through the ManaBox importer, and the
        // rows re-resolve on (set, number, finish) to the same printings.
        let parsed = dumpstar_core::import::manabox::parse(&csv).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].finish, "Foil");
        assert_eq!(parsed[1].finish, "Normal");
        assert_eq!(parsed[1].purchase_price, Some(12.5));
        let report = crate::import::resolve(&conn, &parsed).unwrap();
        assert!(report.unmatched.is_empty(), "{:?}", report.unmatched);
        assert_eq!(report.matched[0].printing_id, "SOR-5-foil");
        assert_eq!(report.matched[1].printing_id, "SOR-5-normal");
    }
}
