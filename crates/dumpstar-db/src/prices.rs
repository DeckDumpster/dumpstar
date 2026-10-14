//! The one rule for "what is this printing worth", and the sealed-product
//! rule beside it.
//!
//! A catalog printing is priced by the newest non-null `market` in the
//! catalog's `prices` table (one row per printing, source and day). A
//! printing a *tenant* invented (`user_printings`) has no feed, and is priced
//! by that tenant's newest `manual_prices` row. The tenant's `manual_prices`
//! can never price a catalog printing: a gap in the feed is the same for every
//! tenant and is the catalog's to fill, and a number only one tenant can see
//! must not change what the shared page means.
//!
//! The same rule is needed three ways, and is spelled once:
//!
//! - today → [`market_price_expr!`], over `prices`, newest row wins;
//! - date D → [`market_price_expr_asof!`], over a `_prices_asof` TEMP relation
//!   `(printing_id, market)` the caller stages with one row per printing, and
//!   with the tenant arm cut off at D.
//!
//! Every expression expects the surrounding query to alias the printing
//! relation `p` and to expose `p.printing_id`.
//!
//! Sealed products price from `sealed_prices` (wide, one row per product and
//! day); see [`sealed_market_price_expr_from!`].

/// The market-price expression over a feed, as a string literal. See the
/// module docs. Callers want [`market_price_expr!`] or
/// [`market_price_expr_asof!`].
#[macro_export]
macro_rules! market_price_expr_from {
    ($feed:expr, $newest:expr, $manual_cutoff:expr) => {
        concat!(
            "COALESCE( \
                 (SELECT lp.market FROM ",
            $feed,
            " lp \
                    WHERE lp.printing_id = p.printing_id \
                      AND lp.market IS NOT NULL ",
            $newest,
            " LIMIT 1), \
                 (SELECT mp.price FROM manual_prices mp \
                    WHERE mp.printing_id = p.printing_id \
                      AND EXISTS (SELECT 1 FROM user_printings up \
                                   WHERE up.printing_id = mp.printing_id) \
                      ",
            $manual_cutoff,
            " ORDER BY mp.observed_at DESC LIMIT 1) \
               )"
        )
    };
}

/// The market price of `p` today.
#[macro_export]
macro_rules! market_price_expr {
    () => {
        $crate::market_price_expr_from!("prices", "ORDER BY lp.observed_on DESC", "")
    };
}

/// The market price of `p` as of date `$date` (a SQL expression such as
/// `"?1"`). The caller must have staged a `_prices_asof` TEMP relation shaped
/// `(printing_id, market)`, one row per printing, its newest quote on or
/// before the date.
#[macro_export]
macro_rules! market_price_expr_asof {
    ($date:expr) => {
        $crate::market_price_expr_from!(
            "_prices_asof",
            "",
            concat!("AND date(mp.observed_at) <= ", $date, " ")
        )
    };
}

/// [`market_price_expr!`] as a value, for code that formats SQL.
pub const MARKET_PRICE_EXPR: &str = crate::market_price_expr!();

/// Effective market price for the **sealed lot** aliased `sc`, exposing
/// `sc.product_id`, resolved against a feed relation shaped like
/// `latest_sealed_prices` — `tcgplayer_product_id`, `market_price`,
/// `mid_price`.
///
/// One arm, not two, and the `COALESCE` inside it is a different thing from
/// the one above: it picks a *price type* off ONE observation, because TCGCSV
/// quotes many sealed products a mid and no market. Arm 2 has no
/// sealed counterpart — `manual_prices` is keyed by `printing_id`, and a
/// sealed lot has none.
///
/// The feed must already be reduced to the newest observation per product:
/// `latest_sealed_prices` is (that view), and the value-history backfill
/// stages `_sealed_prices_asof` the same shape at or before its date. Taking
/// the market from one day and the mid from another would invent a price
/// nobody quoted.
#[macro_export]
macro_rules! sealed_market_price_expr_from {
    ($feed:expr) => {
        concat!(
            "(SELECT COALESCE(lsp.market_price, lsp.mid_price) FROM ",
            $feed,
            " lsp \
                WHERE lsp.tcgplayer_product_id = sc.product_id LIMIT 1)"
        )
    };
}

/// Effective market price for the sealed lot aliased `sc` **right now**.
#[macro_export]
macro_rules! sealed_market_price_expr {
    () => {
        $crate::sealed_market_price_expr_from!("latest_sealed_prices")
    };
}

/// Effective market price for the sealed lot aliased `sc` **as it stood on
/// date D**. The caller must have staged a `_sealed_prices_asof` TEMP
/// relation — `tcgplayer_product_id`, `market_price`, `mid_price` — holding
/// the newest observation at or before D. See
/// [`crate::value_history::backfill`].
#[macro_export]
macro_rules! sealed_market_price_expr_asof {
    () => {
        $crate::sealed_market_price_expr_from!("_sealed_prices_asof")
    };
}

/// Effective market price for the sealed lot aliased `sc`. See
/// [`sealed_market_price_expr_from!`].
pub const SEALED_MARKET_PRICE_EXPR: &str = crate::sealed_market_price_expr!();

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit;
    use crate::{connect_user, open_shared};
    use rusqlite::Connection;

    /// A catalog with two printings and a tenant that invented a third:
    /// `SOR-1-normal`, `SOR-2-normal`, and `SOR-2-user-1`.
    fn db() -> (tempfile::TempDir, std::path::PathBuf, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::simple(&c, "SOR", "1", "Luke Skywalker");
            testkit::simple(&c, "SOR", "2", "Darth Vader");
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        conn.execute(
            "INSERT INTO user_printings (printing_id, card_id, variant, created_at) \
             VALUES ('SOR-2-user-1', 'SOR-2', 'missing_variant', '2026-08-10')",
            [],
        )
        .unwrap();
        (dir, shared, conn)
    }

    fn manual(conn: &Connection, printing: &str, price: f64, at: &str) {
        conn.execute(
            "INSERT INTO manual_prices (printing_id, price, observed_at) VALUES (?1, ?2, ?3)",
            rusqlite::params![printing, price, at],
        )
        .unwrap();
    }

    /// The effective price of one printing, catalog or user-created.
    fn price_of(conn: &Connection, printing: &str) -> Option<f64> {
        conn.query_row(
            &format!(
                "SELECT {MARKET_PRICE_EXPR} FROM \
                   (SELECT printing_id FROM printings \
                    UNION ALL SELECT printing_id FROM user_printings) p \
                 WHERE p.printing_id = ?1"
            ),
            [printing],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn feed_market_prices_a_catalog_printing() {
        let (_d, shared, conn) = db();
        testkit::price(
            &open_shared(&shared).unwrap(),
            "SOR-1-normal",
            "2026-08-01",
            4.5,
        );
        assert_eq!(price_of(&conn, "SOR-1-normal"), Some(4.5));
        // Nothing quoted for the other one: no price, not zero.
        assert_eq!(price_of(&conn, "SOR-2-normal"), None);
    }

    #[test]
    fn newest_feed_day_wins() {
        let (_d, shared, conn) = db();
        let c = open_shared(&shared).unwrap();
        testkit::price(&c, "SOR-1-normal", "2026-08-01", 4.0);
        testkit::price(&c, "SOR-1-normal", "2026-08-03", 6.0);
        testkit::price(&c, "SOR-1-normal", "2026-08-02", 5.0);
        assert_eq!(price_of(&conn, "SOR-1-normal"), Some(6.0));
    }

    #[test]
    fn newest_non_null_market_wins_over_a_newer_empty_quote() {
        let (_d, shared, conn) = db();
        let c = open_shared(&shared).unwrap();
        testkit::price(&c, "SOR-1-normal", "2026-08-01", 4.0);
        // A later day quotes only a mid; its null market must not hide the
        // last real market price.
        c.execute(
            "INSERT INTO prices (printing_id, source, observed_on, mid) \
             VALUES ('SOR-1-normal', 'tcgcsv', '2026-08-02', 9.0)",
            [],
        )
        .unwrap();
        assert_eq!(price_of(&conn, "SOR-1-normal"), Some(4.0));
    }

    #[test]
    fn a_tenant_manual_price_cannot_price_a_catalog_printing() {
        let (_d, shared, conn) = db();
        // A stray row (the insert API refuses it): it must not be read.
        manual(&conn, "SOR-1-normal", 99.0, "2026-08-05T00:00:00Z");
        assert_eq!(price_of(&conn, "SOR-1-normal"), None);
        // Nor does it override the feed.
        testkit::price(
            &open_shared(&shared).unwrap(),
            "SOR-1-normal",
            "2026-08-01",
            4.5,
        );
        assert_eq!(price_of(&conn, "SOR-1-normal"), Some(4.5));
    }

    #[test]
    fn a_user_printing_prices_from_the_newest_manual_row() {
        let (_d, _shared, conn) = db();
        assert_eq!(price_of(&conn, "SOR-2-user-1"), None);
        manual(&conn, "SOR-2-user-1", 100.0, "2026-01-01T00:00:00Z");
        manual(&conn, "SOR-2-user-1", 150.0, "2026-06-01T00:00:00Z");
        manual(&conn, "SOR-2-user-1", 125.0, "2026-03-01T00:00:00Z");
        assert_eq!(price_of(&conn, "SOR-2-user-1"), Some(150.0));
    }
}
