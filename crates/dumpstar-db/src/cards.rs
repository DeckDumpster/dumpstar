//! Read access to the catalog: a card with its printings (owned counts +
//! market price) and the user's owned copies — the card-detail payload.

use rusqlite::{Connection, OptionalExtension, params};

use crate::collection::{self, CollectionEntry};
use crate::error::Result;

/// A catalog card as the detail page shows it. The JSON-typed columns
/// (`aspects`, `traits`, `arenas`) are passed through as raw JSON arrays for
/// the frontend to parse. `set_code`, `number`, `rarity`, `artist` and
/// `image_url` are the *lead printing's* — the one the page was asked for.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct Card {
    pub card_id: String,
    pub set_code: String,
    /// Full set name.
    pub set_name: String,
    pub number: String,
    pub name: String,
    pub subtitle: Option<String>,
    /// `Leader` | `Unit` | `Event` | `Upgrade` | `Base` | `Token`.
    pub card_type: Option<String>,
    #[ts(type = "number | null")]
    pub cost: Option<i64>,
    #[ts(type = "number | null")]
    pub power: Option<i64>,
    #[ts(type = "number | null")]
    pub hp: Option<i64>,
    pub aspects: Option<String>,
    pub traits: Option<String>,
    pub arenas: Option<String>,
    pub rules_text: Option<String>,
    /// The official styled text (HTML with inline game icons) when the
    /// catalog has it; the frontend sanitises it before render.
    pub rules_html: Option<String>,
    pub rarity: Option<String>,
    pub artist: Option<String>,
    pub image_url: Option<String>,
}

/// One printing of a card, with how many copies the user owns and its latest
/// market price.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PrintingInfo {
    pub printing_id: String,
    pub set_code: String,
    pub number: String,
    /// `Standard`, `Hyperspace`, `Showcase`, `Promo`, …
    pub variant: String,
    /// `Normal` or `Foil`.
    pub finish: String,
    pub rarity: Option<String>,
    pub image_url: Option<String>,
    /// The deployed side of a leader printing; `None` for one-sided cards.
    pub back_image_url: Option<String>,
    #[ts(type = "number")]
    pub owned_count: i64,
    pub market_price: Option<f64>,
    /// TCGplayer product id, used to deep-link a printing to its product page.
    #[ts(type = "number | null")]
    pub tcgplayer_product_id: Option<i64>,
    /// True when this row is a user_printings entry — the "Missing
    /// Variant" escape hatch. Renders with an italic + (user) tag in
    /// the UI.
    pub is_user_added: bool,
    /// Free-text variant description carried by user_printings rows.
    pub description: Option<String>,
}

/// The full card-detail payload.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CardDetail {
    pub card: Card,
    pub printings: Vec<PrintingInfo>,
    pub copies: Vec<CollectionEntry>,
}

/// Fetch the card-detail payload for the card printed at a set code and
/// collector number. Returns `None` if no such printing is in the catalog.
pub fn get_card_detail(
    conn: &Connection,
    set_code: &str,
    number: &str,
) -> Result<Option<CardDetail>> {
    let card: Option<Card> = conn
        .prepare(
            "SELECT c.card_id, p.set_code, s.name, p.number, c.name, c.subtitle, c.card_type, \
                    c.cost, c.power, c.hp, c.aspects, c.traits, c.arenas, c.rules_text, \
                    t.html, p.rarity, p.artist, p.image_url \
               FROM printings p \
               JOIN cards c ON c.card_id = p.card_id \
          LEFT JOIN card_text_styled t ON t.card_id = c.card_id \
               JOIN sets s ON s.set_code = p.set_code \
              WHERE p.set_code = ?1 AND p.number = ?2 \
              ORDER BY p.finish <> 'Normal', p.printing_id LIMIT 1",
        )?
        .query_row(params![set_code, number], |r| {
            Ok(Card {
                card_id: r.get(0)?,
                set_code: r.get(1)?,
                set_name: r.get(2)?,
                number: r.get(3)?,
                name: r.get(4)?,
                subtitle: r.get(5)?,
                card_type: r.get(6)?,
                cost: r.get(7)?,
                power: r.get(8)?,
                hp: r.get(9)?,
                aspects: r.get(10)?,
                traits: r.get(11)?,
                arenas: r.get(12)?,
                rules_text: r.get(13)?,
                rules_html: r.get(14)?,
                rarity: r.get(15)?,
                artist: r.get(16)?,
                image_url: r.get(17)?,
            })
        })
        .optional()?;
    let Some(card) = card else {
        return Ok(None);
    };

    let printings: Vec<PrintingInfo> = {
        // UNION ALL of shared.printings and user_printings so the
        // missing-variant escape hatch (decision dumpstar-x7k) shows up
        // alongside catalog rows on /card and CardModal. The binder browse
        // query (binder.rs) deliberately does NOT do this — VariantModal
        // stays catalog-only.
        let mut stmt = conn.prepare(concat!(
            "SELECT p.printing_id, p.set_code, p.number, p.variant, p.finish, p.rarity, \
                    p.image_url, p.back_image_url, \
                    (SELECT count(*) FROM collection c \
                       WHERE c.printing_id = p.printing_id) AS owned_count, \
                    ",
            crate::market_price_expr!(),
            " AS market_price, \
                    (SELECT CAST(psi.source_id AS INTEGER) FROM printing_source_id psi \
                       WHERE psi.printing_id = p.printing_id AND psi.source = 'tcgcsv' \
                       LIMIT 1) AS tcgplayer_product_id, \
                    0 AS is_user_added, \
                    NULL AS description \
             FROM printings p WHERE p.card_id = ?1 \
             UNION ALL \
             SELECT up.printing_id, ?2, ?3, up.variant, 'Normal', NULL, NULL, NULL, \
                    (SELECT count(*) FROM collection c \
                       WHERE c.printing_id = up.printing_id) AS owned_count, \
                    (SELECT mp.price FROM manual_prices mp \
                       WHERE mp.printing_id = up.printing_id \
                       ORDER BY mp.observed_at DESC LIMIT 1) AS market_price, \
                    NULL AS tcgplayer_product_id, \
                    1 AS is_user_added, \
                    up.description \
             FROM user_printings up WHERE up.card_id = ?1 \
             ORDER BY is_user_added, set_code, number, variant, finish, printing_id",
        ))?;
        let rows = stmt.query_map(params![&card.card_id, &card.set_code, &card.number], |r| {
            Ok(PrintingInfo {
                printing_id: r.get(0)?,
                set_code: r.get(1)?,
                number: r.get(2)?,
                variant: r.get(3)?,
                finish: r.get(4)?,
                rarity: r.get(5)?,
                image_url: r.get(6)?,
                back_image_url: r.get(7)?,
                owned_count: r.get(8)?,
                market_price: r.get(9)?,
                tcgplayer_product_id: r.get(10)?,
                is_user_added: r.get::<_, i64>(11)? != 0,
                description: r.get(12)?,
            })
        })?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let copies = collection::list_for_card(conn, &card.card_id)?;
    Ok(Some(CardDetail {
        card,
        printings,
        copies,
    }))
}

/// One snapshot in a printing's market-price history.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PricePoint {
    /// `YYYY-MM-DD` — the day the price was observed.
    pub date: String,
    pub price: f64,
}

/// Full price history for one printing × price_type. The frontend renders
/// one chart line per series.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PriceSeries {
    pub printing_id: String,
    pub variant: String,
    /// `Normal` or `Foil`.
    pub finish: String,
    /// `market` (the feed) or `manual` (hand-entered, for a printing the
    /// tenant invented).
    pub price_type: String,
    pub points: Vec<PricePoint>,
}

/// All price snapshots for every printing of a card, ordered oldest →
/// newest within each series.
///
/// Emits two `price_type`s:
/// - `"market"` — the feed's market price from the catalog `prices` table.
///   A printing with no `market` quote yields no series.
/// - `"manual"` — the tenant's `manual_prices` rows for a printing that
///   tenant invented.
///
/// A tenant `manual_prices` row against a *catalog* printing is deliberately
/// not drawn: nothing reads it, so charting it would show a line that does not
/// price the card.
///
/// For "the effective current value of this printing" the rule is gap-fill:
/// take the latest `market` point if one exists, otherwise the latest
/// `manual` point — the same order [`crate::prices::MARKET_PRICE_EXPR`]
/// applies in SQL. The frontend renders all series and applies that rule when
/// computing summary stats.
pub fn get_card_prices(
    conn: &Connection,
    set_code: &str,
    number: &str,
) -> Result<Vec<PriceSeries>> {
    let card_id: Option<String> = conn
        .prepare("SELECT card_id FROM printings WHERE set_code = ?1 AND number = ?2 LIMIT 1")?
        .query_row(params![set_code, number], |r| r.get(0))
        .optional()?;
    let Some(card_id) = card_id else {
        return Ok(Vec::new());
    };

    let mut stmt = conn.prepare(
        "SELECT p.printing_id AS printing_id, p.variant, p.finish, \
                'market' AS price_type, pr.observed_on AS observed_at, pr.market \
           FROM printings p \
           JOIN prices pr ON pr.printing_id = p.printing_id \
          WHERE p.card_id = ?1 AND pr.market IS NOT NULL \
         UNION ALL \
         SELECT up.printing_id, up.variant, 'Normal' AS finish, \
                'manual' AS price_type, mp.observed_at AS observed_at, mp.price \
           FROM user_printings up \
           JOIN manual_prices mp ON mp.printing_id = up.printing_id \
          WHERE up.card_id = ?1 \
          ORDER BY printing_id, price_type, observed_at",
    )?;

    let mut series: Vec<PriceSeries> = Vec::new();
    let mut rows = stmt.query([&card_id])?;
    while let Some(r) = rows.next()? {
        let printing_id: String = r.get(0)?;
        let variant: String = r.get(1)?;
        let finish: String = r.get(2)?;
        let price_type: String = r.get(3)?;
        let date: String = r.get(4)?;
        let price: f64 = r.get(5)?;

        let point = PricePoint { date, price };
        let key = (&printing_id, &price_type);
        match series
            .last_mut()
            .filter(|s| s.printing_id == *key.0 && s.price_type == *key.1)
        {
            Some(s) => s.points.push(point),
            None => series.push(PriceSeries {
                printing_id,
                variant,
                finish,
                price_type,
                points: vec![point],
            }),
        }
    }
    Ok(series)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::NewCopy;
    use crate::testkit;
    use crate::{connect_user, open_shared};

    /// A catalog with Luke Skywalker (SOR 5): Standard Normal, Standard Foil
    /// and a Hyperspace Normal printing, the lead printing carrying rarity
    /// and artist.
    fn user_conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::card_full(
                &c,
                "SOR-5",
                "Luke Skywalker",
                Some("Jedi Knight"),
                "Unit",
                Some(4),
                Some(4),
                Some(5),
                "[\"Vigilance\",\"Heroism\"]",
                "[\"Force\",\"Rebel\"]",
                "[\"Ground\"]",
                Some("When Played: Give a Shield token to this unit."),
            );
            testkit::printing(
                &c,
                "SOR-5-normal",
                "SOR-5",
                "SOR",
                "5",
                "Standard",
                "Normal",
            );
            testkit::foil_of(&c, "SOR", "5");
            testkit::printing(
                &c,
                "SOR-5-hyper",
                "SOR-5",
                "SOR",
                "255",
                "Hyperspace",
                "Normal",
            );
            c.execute(
                "UPDATE printings SET rarity = 'Legendary', artist = 'Jane Artist', \
                        image_url = 'https://img/luke.png' WHERE printing_id = 'SOR-5-normal'",
                [],
            )
            .unwrap();
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    #[test]
    fn card_detail_includes_printings_and_copies() {
        let (_d, mut conn) = user_conn();
        collection::add(
            &mut conn,
            &NewCopy {
                printing_id: "SOR-5-normal".into(),
                source: "manual_id".into(),
                ..Default::default()
            },
        )
        .unwrap();

        let detail = get_card_detail(&conn, "SOR", "5").unwrap().unwrap();
        assert_eq!(detail.card.name, "Luke Skywalker");
        assert_eq!(detail.card.set_name, "Spark of Rebellion");
        assert_eq!(detail.printings.len(), 3);
        assert_eq!(detail.copies.len(), 1);

        let normal = detail
            .printings
            .iter()
            .find(|p| p.variant == "Standard" && p.finish == "Normal")
            .unwrap();
        assert_eq!(normal.owned_count, 1);
        let foil = detail
            .printings
            .iter()
            .find(|p| p.finish == "Foil")
            .unwrap();
        assert_eq!(foil.owned_count, 0);
        let hyper = detail
            .printings
            .iter()
            .find(|p| p.variant == "Hyperspace")
            .unwrap();
        assert_eq!(hyper.owned_count, 0);
        assert_eq!(hyper.number, "255");
    }

    // The other half of pd-lk8v. The search list stopped shipping the heavy
    // fields because it never drew them; the card modal (CardDetailView, fed
    // by this payload) is what does draw them, one card at a time. If this
    // ever thinned out to match the list, the rules text and the artist
    // facet link would silently go blank.
    #[test]
    fn card_detail_carries_the_full_rules_the_modal_renders() {
        let (_d, conn) = user_conn();
        let detail = get_card_detail(&conn, "SOR", "5").unwrap().unwrap();

        let rules = detail.card.rules_text.as_deref().expect("rules survive");
        assert!(rules.contains("Give a Shield token"));
        assert_eq!(detail.card.subtitle.as_deref(), Some("Jedi Knight"));
        assert_eq!(detail.card.card_type.as_deref(), Some("Unit"));
        assert_eq!(
            (detail.card.cost, detail.card.power, detail.card.hp),
            (Some(4), Some(4), Some(5))
        );
        for (field, part) in [
            (&detail.card.aspects, "Vigilance"),
            (&detail.card.traits, "Rebel"),
            (&detail.card.arenas, "Ground"),
        ] {
            assert!(field.as_deref().unwrap().contains(part));
        }
        // Lead printing's fields.
        assert_eq!(detail.card.number, "5");
        assert_eq!(detail.card.rarity.as_deref(), Some("Legendary"));
        assert_eq!(detail.card.artist.as_deref(), Some("Jane Artist"));
        assert_eq!(
            detail.card.image_url.as_deref(),
            Some("https://img/luke.png")
        );
    }

    #[test]
    fn missing_card_returns_none() {
        let (_d, conn) = user_conn();
        assert!(get_card_detail(&conn, "SOR", "999").unwrap().is_none());
    }

    #[test]
    fn card_detail_resolves_per_printing_market_price() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "6", "Millennium Falcon");
            testkit::foil_of(&c, "SOR", "6");
            testkit::price(&c, "SOR-6-normal", "2026-05-17", 9.0);
            testkit::price(&c, "SOR-6-normal", "2026-05-18", 10.0);
            testkit::price(&c, "SOR-6-foil", "2026-05-18", 25.0);
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let detail = get_card_detail(&conn, "SOR", "6").unwrap().unwrap();

        let normal = detail
            .printings
            .iter()
            .find(|p| p.finish == "Normal")
            .unwrap();
        let foil = detail
            .printings
            .iter()
            .find(|p| p.finish == "Foil")
            .unwrap();
        // The newest quote wins.
        assert_eq!(normal.market_price, Some(10.0));
        assert_eq!(foil.market_price, Some(25.0));
    }

    #[test]
    fn card_detail_leaves_an_unpriced_catalog_printing_null_and_gap_fills_only_user_printings() {
        // A catalog printing prices identically for every tenant: with no
        // feed quote it is NULL, and a stray tenant manual price against it
        // does not fill the gap. A printing the tenant invented has no feed
        // to wait for, so its newest manual price stands.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "10", "Vanquish");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let detail = get_card_detail(&conn, "SOR", "10").unwrap().unwrap();
        assert_eq!(detail.printings[0].market_price, None);

        conn.execute(
            "INSERT INTO manual_prices (printing_id, price, observed_at) \
             VALUES ('SOR-10-normal', 99.0, '2026-05-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO user_printings (printing_id, card_id, variant, description) \
             VALUES ('SOR-10-user-1', 'SOR-10', 'missing_variant', 'Convention stamp')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO manual_prices (printing_id, price, observed_at) \
             VALUES ('SOR-10-user-1', 12.5, '2026-05-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let detail = get_card_detail(&conn, "SOR", "10").unwrap().unwrap();
        let catalog = detail
            .printings
            .iter()
            .find(|p| p.printing_id == "SOR-10-normal")
            .unwrap();
        assert_eq!(catalog.market_price, None, "tenant price cannot reach it");
        assert!(!catalog.is_user_added);
        let user = detail
            .printings
            .iter()
            .find(|p| p.printing_id == "SOR-10-user-1")
            .unwrap();
        assert_eq!(user.market_price, Some(12.5));
        assert!(user.is_user_added);
        assert_eq!(user.description.as_deref(), Some("Convention stamp"));
    }

    #[test]
    fn get_card_prices_returns_ordered_series_per_printing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "6", "Millennium Falcon");
            testkit::foil_of(&c, "SOR", "6");
            // Two days per finish, inserted out of order, plus a quote with
            // no market that must be filtered out.
            testkit::price(&c, "SOR-6-normal", "2026-05-22", 10.0);
            testkit::price(&c, "SOR-6-normal", "2026-05-21", 9.5);
            testkit::price(&c, "SOR-6-foil", "2026-05-21", 24.0);
            testkit::price(&c, "SOR-6-foil", "2026-05-22", 25.0);
            c.execute(
                "INSERT INTO prices (printing_id, source, observed_on, low) \
                 VALUES ('SOR-6-normal', 'tcgcsv', '2026-05-23', 7.0)",
                [],
            )
            .unwrap();
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let series = get_card_prices(&conn, "SOR", "6").unwrap();
        assert_eq!(series.len(), 2, "one series per printing");
        let normal = series.iter().find(|s| s.finish == "Normal").unwrap();
        assert_eq!(normal.variant, "Standard");
        assert_eq!(normal.points.len(), 2);
        assert_eq!(normal.points[0].date, "2026-05-21");
        assert_eq!(normal.points[0].price, 9.5);
        assert_eq!(normal.points[1].price, 10.0);
        let foil = series.iter().find(|s| s.finish == "Foil").unwrap();
        assert_eq!(foil.points[1].price, 25.0);
        assert!(series.iter().all(|s| s.price_type == "market"));
    }

    #[test]
    fn get_card_prices_returns_empty_for_unknown_card() {
        let (_d, conn) = user_conn();
        assert!(get_card_prices(&conn, "SOR", "999").unwrap().is_empty());
    }

    /// A catalog with `SOR-14-normal` and no price quote at all.
    fn unpriced_catalog(dir: &std::path::Path) -> std::path::PathBuf {
        let shared = dir.join("shared.sqlite");
        let c = open_shared(&shared).unwrap();
        testkit::simple(&c, "SOR", "14", "Darth Vader");
        shared
    }

    #[test]
    fn get_card_prices_has_no_series_for_a_catalog_printing_without_a_quote() {
        let dir = tempfile::tempdir().unwrap();
        let shared = unpriced_catalog(dir.path());
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        assert!(get_card_prices(&conn, "SOR", "14").unwrap().is_empty());
    }

    /// A tenant that holds a manual price against a catalog printing. Nothing
    /// reads it, so the chart must not draw it either — a line that does not
    /// price the card is worse than no line.
    #[test]
    fn get_card_prices_ignores_a_stale_tenant_manual_price_on_a_catalog_printing() {
        let dir = tempfile::tempdir().unwrap();
        let shared = unpriced_catalog(dir.path());
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        // Written straight to the table, since `manual_prices::insert`
        // refuses this printing.
        conn.execute(
            "INSERT INTO manual_prices (printing_id, price, observed_at) \
             VALUES ('SOR-14-normal', 200.0, '2024-06-01T00:00:00Z')",
            [],
        )
        .unwrap();

        assert!(get_card_prices(&conn, "SOR", "14").unwrap().is_empty());
    }

    /// …while a printing the tenant invented still charts from its own rows,
    /// alongside the catalog printing's market series.
    #[test]
    fn get_card_prices_charts_a_user_created_printings_manual_series() {
        let dir = tempfile::tempdir().unwrap();
        let shared = unpriced_catalog(dir.path());
        {
            let c = open_shared(&shared).unwrap();
            testkit::price(&c, "SOR-14-normal", "2026-05-22", 3.0);
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        conn.execute(
            "INSERT INTO user_printings (printing_id, card_id, variant, created_at) \
             VALUES ('SOR-14-user-1', 'SOR-14', 'missing_variant', '2026-08-10')",
            [],
        )
        .unwrap();
        for (t, p) in [
            ("2024-06-01T00:00:00Z", 200.0),
            ("2024-03-01T00:00:00Z", 175.0),
        ] {
            crate::manual_prices::insert(
                &conn,
                &crate::manual_prices::NewManualPrice {
                    printing_id: "SOR-14-user-1".into(),
                    price: p,
                    observed_at: Some(t.into()),
                    note: None,
                },
            )
            .unwrap();
        }

        let series = get_card_prices(&conn, "SOR", "14").unwrap();
        assert_eq!(series.len(), 2);
        let market = series.iter().find(|s| s.price_type == "market").unwrap();
        assert_eq!(market.printing_id, "SOR-14-normal");
        assert_eq!(market.points[0].price, 3.0);
        let s = series.iter().find(|s| s.price_type == "manual").unwrap();
        assert_eq!(s.printing_id, "SOR-14-user-1");
        assert_eq!(s.points.len(), 2);
        // Oldest first within the series.
        assert_eq!(s.points[0].price, 175.0);
        assert_eq!(s.points[1].price, 200.0);
    }
}
