//! `dumpstar seed-fixture` — build the deterministic Star Wars: Unlimited test fixture for
//! the intents UI-testing harness.
//!
//! Produces two SQLite files in `--out` (default `tests/ui/fixtures`):
//!
//! * `shared.sqlite` — immutable catalog (sets, cards, printings, prices).
//! * `collection.sqlite` — per-user mutable data (binders, decks, batches,
//!   orders, sealed, wishlist, views, collection).
//!
//! Both databases are built through Dumpstar's own code so the schema
//! init runs and the running server can open them unchanged. The intents
//! harness snapshots/restores these before each test.
//!
//! The data is small, deterministic, and drawn from the catalog's Star Wars:
//! Unlimited fake source (`crates/dumpstar-catalog/fake/swu-fake.json`);
//! intent YAML/hint files reference it by the stable names documented in
//! `tests/ui/fixtures/README.md`.

use std::path::PathBuf;

use rusqlite::Connection;

use dumpstar_db::{
    batches::{self, NewBatch},
    binders::{self, NewBinder},
    collection::{self, NewCopy},
    decks::{self, NewDeck},
    orders::{self, NewOrder, OrderLine},
    sealed::{self, NewSealed},
    wishlist::{self, NewWish},
};

/// Arguments for `dumpstar seed-fixture`.
#[derive(clap::Args)]
pub struct FixtureArgs {
    /// Output directory for `shared.sqlite` and `collection.sqlite`.
    #[arg(long, value_name = "DIR", default_value = "tests/ui/fixtures")]
    out: PathBuf,
}

/// The instant the fixture's own story starts, and the step the seeding clock
/// takes between one stamped row and the next.
///
/// Every timestamp in the fixture that is not a hand-written constant is
/// `SEEDED_AT + n * SEEDING_STEP`, so two regenerations of an unchanged seeder
/// produce the same bytes — see [`run`].
///
/// The date is chosen, not arbitrary. It is the fixture's own `OBSERVED_AT`,
/// the day its prices were quoted; it sits after the last order date the
/// fixture invents (2024-01-14) and after the acquisition constant on the
/// manually-entered copies (2024-01-10), so the collection reads as one that
/// was assembled and then priced. It also sits well before the instant
/// `tests/visual/stabilize.ts` freezes the browser at (2026-01-15), which the
/// build timestamps it replaces did not — those were EIGHT MONTHS in the
/// future, so anything rendering an age against the frozen clock rendered a
/// negative one.
///
/// A minute per step, rather than a second, because `/recent` and `/batches`
/// render `created_at.slice(0, 16)` — to the minute. A finer step is stable in
/// the file and identical on the screen, and a baseline showing five batches
/// all stamped `09:00` demonstrates nothing about the ordering those two
/// routes exist to show.
const SEEDED_AT: &str = "2024-01-15T09:00:00Z";
const SEEDING_STEP: chrono::TimeDelta = chrono::TimeDelta::minutes(1);

/// Execute `dumpstar seed-fixture`.
///
/// The output is BYTE-STABLE: seeding twice produces two identical pairs of
/// files, and `tests::two_seed_runs_are_byte_identical` is the gate that says
/// so. That is what the pinned clock below is for. The fixture is a committed
/// binary artefact, so a regeneration that rewrites it with a new afternoon in
/// it is a diff nobody can review, and the visual baselines of the four routes
/// that render these columns — `/recent`, `/batches`, `/batches/[id]`,
/// `/sealed` — move with it at both viewports. The cost is not the churn: it
/// is that a real regression arrives inside the churn and gets waved through
/// with it (pd-nzlj).
pub fn run(args: FixtureArgs) -> anyhow::Result<()> {
    // The rows go in through the repository functions on purpose, so the
    // app-layer validation runs — and those functions stamp their timestamp
    // columns from the clock. Pin it for the length of the build: a timeline
    // that starts at a constant and advances one step per read, so the rows
    // come out in the order they always did, at instants that are the same
    // every time. Advancing rather than frozen because `/recent` and
    // `/batches` ORDER BY these columns, and one constant would leave their
    // order a tie broken by rowid.
    dumpstar_db::clock::pin(
        SEEDED_AT.parse::<chrono::DateTime<chrono::Utc>>()?,
        SEEDING_STEP,
    );
    let out = seed(args);
    dumpstar_db::clock::unpin();
    out
}

fn seed(args: FixtureArgs) -> anyhow::Result<()> {
    std::fs::create_dir_all(&args.out)?;
    let shared_path = args.out.join("shared.sqlite");
    let user_path = args.out.join("collection.sqlite");

    // Clean rebuild — stale rows would break determinism, even though
    // the new IF NOT EXISTS schema would otherwise let us reuse the file.
    for p in [&shared_path, &user_path] {
        if p.exists() {
            std::fs::remove_file(p)?;
        }
    }

    println!("Building shared catalog at {}", shared_path.display());
    let shared = dumpstar_db::open_shared(&shared_path)?;
    seed_catalog(&shared)?;
    // The keyword registry, rarity ranks and flag definitions fixture-backed
    // search tests need came in with the read-write open above
    // (connection.rs::converge).
    // Materialize latest_prices from the seeded prices (vi37) so fixture-backed
    // pages show market values without re-running a full catalog refresh.
    dumpstar_db::latest_prices::refresh_latest_prices(&shared)?;
    let (sets, cards, printings, prices) = catalog_counts(&shared)?;
    println!("  {sets} sets, {cards} cards, {printings} printings, {prices} prices");
    drop(shared);

    println!("Building user collection at {}", user_path.display());
    let mut user = dumpstar_db::connect_user(&user_path, &shared_path)?;
    seed_user(&mut user)?;
    let copies: i64 = user.query_row("SELECT count(*) FROM collection", [], |r| r.get(0))?;
    println!("  {copies} collection copies seeded");
    // Reconstruct value-history snapshots from the seeded prices + copies so
    // fixture-backed value-chart tests have data (deterministic: fixed prices,
    // fixed acquired_at). (dumpstar-e1vo)
    let snaps = dumpstar_db::value_history::backfill(&mut user)?;
    println!("  {snaps} value-history snapshot rows");

    let events = date_the_outbox(&user)?;
    println!("  {events} ownership-outbox events dated from their payloads");

    // VACUUM last, and it is part of the determinism rather than tidiness: a
    // page freed during the build keeps its old contents until something
    // reuses it, so two runs whose every row was identical still differed in
    // the bytes of the pre-dating outbox rows left lying in the file. A
    // vacuumed database is written from its own content and nothing else.
    user.execute_batch("VACUUM")?;

    println!("Fixture ready: {}", args.out.display());
    Ok(())
}

/// Put the ownership outbox on the fixture's own timeline.
///
/// The pinned clock cannot reach these rows: the outbox is written by the
/// triggers in `schema_user.sql`, inside the statement's own transaction, and
/// they read SQLite's clock rather than this process's. That is the property
/// the outbox is built on — a trigger fires where no call site can forget it —
/// and it is not one to weaken for a fixture.
///
/// So each event is dated afterwards from the holding it describes, which is
/// the honest answer anyway: `payload` is the whole row as JSON, so the row's
/// own `acquired_at` / `added_at` is in there for every event, including a
/// delete whose row is already gone. `seq` breaks the ties in milliseconds and
/// stays the ordering authority, exactly as the schema says it is.
///
/// Nothing renders the outbox; this is for byte-stability, and for a fixture
/// whose transport log agrees with its own story rather than claiming every
/// holding was shipped at 22:03 on the afternoon somebody rebuilt it. Updating
/// this table fires no trigger — the three per source are on `collection` and
/// `sealed_collection` — so dating it appends no events of its own.
fn date_the_outbox(conn: &Connection) -> anyhow::Result<usize> {
    let n = conn.execute(
        "UPDATE ownership_outbox SET occurred_at = strftime( \
             '%Y-%m-%dT%H:%M:%fZ', \
             COALESCE(json_extract(payload, '$.acquired_at'), \
                      json_extract(payload, '$.added_at')), \
             '+' || (seq / 1000.0) || ' seconds')",
        [],
    )?;
    // A source whose payload carried neither column would leave a NULL and
    // fail the NOT NULL rather than pass quietly — but an outbox that is
    // EMPTY would satisfy the statement whatever it does.
    assert!(n > 0, "the fixture seeded no ownership-outbox events");
    Ok(n)
}

fn catalog_counts(conn: &Connection) -> anyhow::Result<(i64, i64, i64, i64)> {
    Ok((
        conn.query_row("SELECT count(*) FROM sets", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM cards", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM printings", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM prices", [], |r| r.get(0))?,
    ))
}

// ---------------------------------------------------------------------------
// Catalog (shared.sqlite)
// ---------------------------------------------------------------------------
//
// The Star Wars: Unlimited cards, printings and prices below are the ones in
// the catalog's fake source, `crates/dumpstar-catalog/fake/swu-fake.json`:
// two sets, three cards, and their Standard / Hyperspace / Showcase / Promo
// printings in the Normal and Foil finishes. Each distinct collector number
// is one `cards` row; each finish is one `printings` row, the foil finish
// under the `holo` variant code the importers map `Foil` to.

/// A fixed observation date so prices are byte-for-byte reproducible.
const OBSERVED_AT: &str = "2024-01-15";
/// A fixed ingest timestamp for the catalog freshness markers.
const FETCHED_AT: &str = "2024-01-15T00:00:00Z";

/// One collector-numbered card.
struct CardSpec {
    set_code: &'static str,
    number: &'static str,
    sortable: i64,
    name: &'static str,
    card_type: &'static str,
    rarity: &'static str,
    artist: Option<&'static str>,
    hp: Option<i64>,
    /// JSON array of aspects.
    aspects: &'static str,
    /// JSON array of traits.
    traits: &'static str,
}

/// One finish of a card, with its optional TCGplayer product link.
struct PrintingSpec {
    variant: &'static str,
    sub_type: Option<&'static str>,
    product_id: Option<i64>,
    /// `Some` writes a `prices` row at [`OBSERVED_AT`].
    market_price: Option<f64>,
}

/// A card paired with the printings to materialise for it.
type CardEntry = (CardSpec, Vec<PrintingSpec>);

/// A set row: set_code, name, set_sort, release_date.
type SetRow = (&'static str, &'static str, i64, &'static str);

/// A sealed-product row: product_id, set_code, name, category, card_count,
/// product_size, release_date.
type SealedRow = (
    i64,
    &'static str,
    &'static str,
    &'static str,
    Option<i64>,
    Option<i64>,
    &'static str,
);

/// A sealed-price row: product_id, low, mid, high, market, direct_low.
///
/// `market` is `Option` because TCGCSV genuinely quotes many sealed products a
/// mid and no market, and `sealed_market_price_expr_from!` exists to pick
/// between the two off ONE observation. A fixture that filled `market` on
/// every row would leave that `COALESCE` unexercised by anything a screenshot
/// can see.
type SealedPriceRow = (i64, f64, f64, f64, Option<f64>, Option<f64>);

fn seed_catalog(conn: &Connection) -> anyhow::Result<()> {
    // --- Sets ----------------------------------------------------------
    let sets: [SetRow; 2] = [
        ("SOR", "Spark of Rebellion", 1, "2024-03-08"),
        ("SHD", "Shadows of the Galaxy", 2, "2024-07-12"),
    ];
    for (code, name, sort, release) in sets {
        conn.execute(
            "INSERT INTO sets \
               (set_code, ptcgo_code, name, series, set_sort_order, \
                release_date, ptcgio_fetched_at) \
             VALUES (?1, ?1, ?2, 'Star Wars: Unlimited', ?3, ?4, ?5)",
            rusqlite::params![code, name, sort, release, FETCHED_AT],
        )?;
    }

    // --- Cards + printings + prices ------------------------------------
    seed_cards(conn, &catalog_cards())?;

    // --- Sealed products ----------------------------------------------
    // (product_id, set_code, name, category, card_count, product_size,
    //  release_date)
    let sealed: [SealedRow; 4] = [
        (
            910001,
            "SOR",
            "Spark of Rebellion Booster Display",
            "booster_display",
            Some(16),
            Some(24),
            "2024-03-08",
        ),
        (
            910002,
            "SOR",
            "Spark of Rebellion Two-Player Starter",
            "two_player_starter",
            Some(10),
            Some(9),
            "2024-03-08",
        ),
        (
            910003,
            "SOR",
            "Spark of Rebellion Prerelease Kit",
            "prerelease_kit",
            Some(10),
            Some(6),
            "2024-03-08",
        ),
        (
            910004,
            "SHD",
            "Shadows of the Galaxy Booster Pack",
            "booster_pack",
            Some(16),
            Some(1),
            "2024-07-12",
        ),
    ];
    for (pid, set, name, cat, cc, size, release) in sealed {
        conn.execute(
            "INSERT INTO sealed_products \
               (product_id, set_code, name, category, card_count, \
                product_size, release_date, image_url, tcgplayer_url, fetched_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                pid,
                set,
                name,
                cat,
                cc,
                size,
                release,
                format!("https://product-images.tcgplayer.com/{pid}.jpg"),
                format!("https://www.tcgplayer.com/product/{pid}"),
                FETCHED_AT,
            ],
        )?;
    }

    // --- Sealed prices -------------------------------------------------
    //
    // The two products the collection actually HOLDS, quoted one way each, so
    // the rendered sealed value goes through the whole rule rather than one
    // arm of it (pd-bbv7, sp-ysb):
    //
    //   910002  a full quote, `market_price` set — the ordinary arm
    //   910004  `market_price` NULL, mid alone — the `COALESCE` fallback
    //
    // 910001 and 910003 stay unquoted deliberately. Neither is held, so
    // neither moves a number, and leaving them keeps the catalog carrying the
    // shape TCGCSV really produces: a sealed product with no price row at all.
    //
    // Fixed constants at `OBSERVED_AT`, like every other price here. The two
    // owned lots then value at 59.42 + 6 x 5.24 = 90.86 against the 76.93 they
    // cost, so the sealed series reads as a small gain rather than as a round
    // number nobody would notice was wrong.
    //
    // (product_id, low, mid, high, market, direct_low)
    let sealed_prices: [SealedPriceRow; 2] = [
        (910002, 52.00, 58.75, 79.99, Some(59.42), Some(55.10)),
        (910004, 4.10, 5.24, 8.99, None, None),
    ];
    for (pid, low, mid, high, market, direct_low) in sealed_prices {
        conn.execute(
            "INSERT INTO sealed_prices \
               (tcgplayer_product_id, low_price, mid_price, high_price, \
                market_price, direct_low_price, observed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![pid, low, mid, high, market, direct_low, OBSERVED_AT],
        )?;
    }

    Ok(())
}

/// Insert the cards, their printings, and a price row per quoted printing.
fn seed_cards(conn: &Connection, cards: &[CardEntry]) -> anyhow::Result<()> {
    for (card, printings) in cards {
        let card_id = format!("{}-{}", card.set_code, card.number);
        conn.execute(
            "INSERT INTO cards \
               (card_id, set_code, number, number_sortable, name, supertype, \
                subtypes, types, rarity, artist, hp) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                card_id,
                card.set_code,
                card.number,
                card.sortable,
                card.name,
                card.card_type,
                card.traits,
                card.aspects,
                card.rarity,
                card.artist,
                card.hp,
            ],
        )?;

        for p in printings {
            let printing_id = format!("{card_id}-{}", p.variant);
            // sub_type_name links a printing to its prices row (the market_price
            // join in search.rs keys on it). Production ingest populates it; the
            // fixture must too, or every price reads back null (dumpstar-qm9).
            conn.execute(
                "INSERT INTO printings \
                   (printing_id, card_id, variant, language, tcgplayer_product_id, \
                    sub_type_name) \
                 VALUES (?1, ?2, ?3, 'en', ?4, ?5)",
                rusqlite::params![printing_id, card_id, p.variant, p.product_id, p.sub_type],
            )?;

            if let (Some(pid), Some(sub), Some(price)) = (p.product_id, p.sub_type, p.market_price)
            {
                conn.execute(
                    "INSERT INTO prices \
                       (tcgplayer_product_id, sub_type_name, source, \
                        price_type, price, observed_at) \
                     VALUES (?1, ?2, 'tcgplayer', 'market', ?3, ?4)",
                    rusqlite::params![pid, sub, price, OBSERVED_AT],
                )?;
            }
        }
    }
    Ok(())
}

/// The Normal finish of a product, quoted at `price` when `Some`.
fn normal(product_id: Option<i64>, price: Option<f64>) -> PrintingSpec {
    PrintingSpec {
        variant: "normal",
        sub_type: product_id.map(|_| "Normal"),
        product_id,
        market_price: price,
    }
}

/// The Foil finish of a product, quoted at `price` when `Some`.
fn foil(product_id: i64, price: Option<f64>) -> PrintingSpec {
    PrintingSpec {
        variant: "holo",
        sub_type: Some("Foil"),
        product_id: Some(product_id),
        market_price: price,
    }
}

/// The fake source's cards, one row per collector number.
fn catalog_cards() -> Vec<CardEntry> {
    vec![
        (
            CardSpec {
                set_code: "SOR",
                number: "5",
                sortable: 5,
                name: "Luke Skywalker - Faithful Friend",
                card_type: "Leader",
                rarity: "Special",
                artist: Some("Borja Pindado"),
                hp: Some(7),
                aspects: r#"["Vigilance","Heroism"]"#,
                traits: r#"["Force","Rebel"]"#,
            },
            vec![normal(Some(900001), Some(4.60)), foil(900001, Some(24.00))],
        ),
        (
            CardSpec {
                set_code: "SOR",
                number: "262",
                sortable: 262,
                name: "Luke Skywalker - Faithful Friend (Hyperspace)",
                card_type: "Leader",
                rarity: "Special",
                artist: Some("Borja Pindado"),
                hp: Some(7),
                aspects: r#"["Vigilance","Heroism"]"#,
                traits: r#"["Force","Rebel"]"#,
            },
            vec![normal(Some(900002), None), foil(900002, None)],
        ),
        (
            CardSpec {
                set_code: "SOR",
                number: "193",
                sortable: 193,
                name: "Millennium Falcon - Piece of Junk",
                card_type: "Unit",
                rarity: "Rare",
                artist: Some("Matt Ferguson"),
                hp: Some(4),
                aspects: r#"["Cunning"]"#,
                traits: r#"["Vehicle","Transport"]"#,
            },
            vec![normal(Some(900003), None)],
        ),
        (
            CardSpec {
                set_code: "SHD",
                number: "500",
                sortable: 500,
                name: "Millennium Falcon - Piece of Junk (Showcase)",
                card_type: "Unit",
                rarity: "Legendary",
                artist: Some("Kuroimori"),
                hp: Some(4),
                aspects: r#"["Cunning"]"#,
                traits: r#"["Vehicle","Transport"]"#,
            },
            vec![normal(Some(900004), Some(75.00))],
        ),
        (
            CardSpec {
                set_code: "SOR",
                number: "P01",
                sortable: 900_001,
                name: "Vanquish",
                card_type: "Event",
                rarity: "Common",
                artist: None,
                hp: None,
                aspects: r#"["Command"]"#,
                traits: "[]",
            },
            vec![normal(None, None)],
        ),
    ]
}

// ---------------------------------------------------------------------------
// User data (collection.sqlite)
// ---------------------------------------------------------------------------

fn seed_user(conn: &mut Connection) -> anyhow::Result<()> {
    // --- Binders -------------------------------------------------------
    let trade_binder = binders::create(
        conn,
        &NewBinder {
            name: "Trade Binder".into(),
            description: Some("Spare cards available for trade.".into()),
            color: "red".into_some(),
            binder_type: "trade".into_some(),
            pocket_size: Some(9),
            storage_location: "Shelf A".into_some(),
        },
    )?;
    let masters = binders::create(
        conn,
        &NewBinder {
            name: "Master Set: Spark of Rebellion".into(),
            description: Some("Working toward a complete Spark of Rebellion master set.".into()),
            color: "blue".into_some(),
            binder_type: "set".into_some(),
            pocket_size: Some(12),
            storage_location: "Shelf A".into_some(),
        },
    )?;
    let vault = binders::create(
        conn,
        &NewBinder {
            name: "Showcase Vault".into(),
            description: Some("High-value Showcase and Hyperspace printings.".into()),
            color: "gold".into_some(),
            binder_type: "showcase".into_some(),
            pocket_size: Some(9),
            storage_location: "Safe".into_some(),
        },
    )?;

    // --- Decks (idea / ready / built) ----------------------------------
    let luke_deck = decks::create(
        conn,
        &NewDeck {
            name: "Luke Skywalker Vigilance".into(),
            description: Some("Vigilance/Heroism control led by Luke Skywalker.".into()),
            format: "premier".into_some(),
            owner: "Ryan".into_some(),
            state: "built".into_some(),
            sleeve_color: "black".into_some(),
            storage_location: "Deck Box 1".into_some(),
            notes: "Tournament-ready.".into_some(),
        },
    )?;
    let _falcon_deck = decks::create(
        conn,
        &NewDeck {
            name: "Millennium Falcon Cunning".into(),
            description: Some("Fast Cunning aggro around the Falcon.".into()),
            format: "premier".into_some(),
            owner: "Ryan".into_some(),
            state: "ready".into_some(),
            sleeve_color: "yellow".into_some(),
            storage_location: "Deck Box 2".into_some(),
            notes: None,
        },
    )?;
    let _twin_suns_deck = decks::create(
        conn,
        &NewDeck {
            name: "Twin Suns Brawl".into(),
            description: Some("A casual Twin Suns deck idea.".into()),
            format: "twin_suns".into_some(),
            owner: "Alice".into_some(),
            state: "idea".into_some(),
            sleeve_color: None,
            storage_location: None,
            notes: "Needs more units.".into_some(),
        },
    )?;

    // --- Orders --------------------------------------------------------
    // One received order and one still-open order. `orders::create` seeds
    // its own batch + `ordered` collection rows; `receive` flips them owned.
    let received_order = orders::create(
        conn,
        &NewOrder {
            order_number: "TCG-100001".into_some(),
            source: "tcgplayer".into(),
            seller_name: "CardKingdomLA".into_some(),
            order_date: "2024-01-05".into_some(),
            subtotal: Some(31.50),
            shipping: Some(4.99),
            tax: Some(2.84),
            total: Some(39.33),
            shipping_status: "delivered".into_some(),
            estimated_delivery: "2024-01-12".into_some(),
            notes: "Singles for the Luke deck.".into_some(),
        },
        &[
            OrderLine {
                printing_id: "SOR-5-holo".into(),
                quantity: 1,
                purchase_price: Some(24.00),
            },
            OrderLine {
                printing_id: "SOR-193-normal".into(),
                quantity: 2,
                purchase_price: Some(1.00),
            },
        ],
    )?;
    orders::receive(conn, received_order)?;

    let _open_order = orders::create(
        conn,
        &NewOrder {
            order_number: "EBAY-55012".into_some(),
            source: "ebay".into(),
            seller_name: "outer_rim_cards".into_some(),
            order_date: "2024-01-14".into_some(),
            subtotal: Some(80.00),
            shipping: Some(0.00),
            tax: Some(0.00),
            total: Some(80.00),
            shipping_status: "shipped".into_some(),
            estimated_delivery: "2024-01-22".into_some(),
            notes: "Showcase Millennium Falcon — still in transit.".into_some(),
        },
        &[OrderLine {
            printing_id: "SHD-500-normal".into(),
            quantity: 1,
            purchase_price: Some(80.00),
        }],
    )?;

    // --- Batches -------------------------------------------------------
    // (Orders already created their own batches; these are the manual /
    // binder-click / CSV ingest batches.)
    let manual_batch = batches::create(
        conn,
        &NewBatch {
            batch_type: "manual_id".into(),
            name: "Showcase entry".into_some(),
            notes: "Hand-entered Showcase and Hyperspace printings.".into_some(),
            order_id: None,
            binder_id: Some(vault),
        },
    )?;
    let click_batch = batches::create(
        conn,
        &NewBatch {
            batch_type: "binder_click".into(),
            name: "Spark of Rebellion binder page-through".into_some(),
            notes: "Registered via the binder-page browser.".into_some(),
            order_id: None,
            binder_id: Some(masters),
        },
    )?;
    let csv_batch = batches::create(
        conn,
        &NewBatch {
            batch_type: "csv_manabox".into(),
            name: "ManaBox export 2024-01".into_some(),
            notes: "Bulk import from a ManaBox CSV.".into_some(),
            order_id: None,
            binder_id: Some(trade_binder),
        },
    )?;

    // --- Collection copies --------------------------------------------
    // Each tuple: printing_id, condition, status, source, binder, deck,
    // batch, purchase_price, notes.
    struct Copy {
        printing_id: &'static str,
        condition: &'static str,
        status: &'static str,
        source: &'static str,
        binder: Option<i64>,
        deck: Option<i64>,
        batch: Option<i64>,
        price: Option<f64>,
        notes: Option<&'static str>,
    }
    let copies = vec![
        // Showcase and Hyperspace printings — manual batch, Showcase Vault binder.
        Copy {
            printing_id: "SHD-500-normal",
            condition: "Lightly Played",
            status: "owned",
            source: "manual_id",
            binder: Some(vault),
            deck: None,
            batch: Some(manual_batch),
            price: Some(280.00),
            notes: Some("First Showcase pull — well loved."),
        },
        Copy {
            printing_id: "SOR-262-holo",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: Some(vault),
            deck: None,
            batch: Some(manual_batch),
            price: Some(140.00),
            notes: None,
        },
        Copy {
            printing_id: "SOR-262-normal",
            condition: "Moderately Played",
            status: "owned",
            source: "manual_id",
            binder: Some(vault),
            deck: None,
            batch: Some(manual_batch),
            price: Some(95.00),
            notes: None,
        },
        Copy {
            printing_id: "SOR-5-holo",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: Some(vault),
            deck: None,
            batch: Some(manual_batch),
            price: Some(22.00),
            notes: None,
        },
        // A graded card (set via direct UPDATE after insert).
        Copy {
            printing_id: "SOR-193-normal",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: Some(vault),
            deck: None,
            batch: Some(manual_batch),
            price: Some(60.00),
            notes: Some("PSA 8 slab."),
        },
        // Spark of Rebellion binder-click registrations — Master Set binder.
        Copy {
            printing_id: "SOR-5-normal",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-5-holo",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-193-normal",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-5-normal",
            condition: "Lightly Played",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-P01-normal",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: Some(3.00),
            notes: None,
        },
        Copy {
            printing_id: "SOR-262-holo",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: Some(8.50),
            notes: Some("Hyperspace Luke in foil."),
        },
        Copy {
            printing_id: "SOR-262-normal",
            condition: "Near Mint",
            status: "owned",
            source: "binder_click",
            binder: Some(masters),
            deck: None,
            batch: Some(click_batch),
            price: Some(95.00),
            notes: Some("Hyperspace Luke — chase card."),
        },
        // ManaBox CSV import — Trade Binder.
        Copy {
            printing_id: "SOR-193-normal",
            condition: "Near Mint",
            status: "owned",
            source: "csv_manabox",
            binder: Some(trade_binder),
            deck: None,
            batch: Some(csv_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-P01-normal",
            condition: "Near Mint",
            status: "owned",
            source: "csv_manabox",
            binder: Some(trade_binder),
            deck: None,
            batch: Some(csv_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SOR-5-normal",
            condition: "Lightly Played",
            status: "owned",
            source: "csv_manabox",
            binder: Some(trade_binder),
            deck: None,
            batch: Some(csv_batch),
            price: None,
            notes: None,
        },
        Copy {
            printing_id: "SHD-500-normal",
            condition: "Near Mint",
            status: "listed",
            source: "csv_manabox",
            binder: Some(trade_binder),
            deck: None,
            batch: Some(csv_batch),
            price: Some(18.00),
            notes: Some("Listed for trade — Showcase Falcon."),
        },
        // Cards built into the Luke Skywalker Vigilance deck.
        Copy {
            printing_id: "SOR-5-holo",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: None,
            deck: Some(luke_deck),
            batch: None,
            price: Some(24.00),
            notes: Some("Deck centrepiece."),
        },
        Copy {
            printing_id: "SOR-193-normal",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: None,
            deck: Some(luke_deck),
            batch: None,
            price: Some(1.00),
            notes: None,
        },
        Copy {
            printing_id: "SOR-P01-normal",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: None,
            deck: Some(luke_deck),
            batch: None,
            price: None,
            notes: None,
        },
        // Loose owned copies — unassigned.
        Copy {
            printing_id: "SOR-5-holo",
            condition: "Near Mint",
            status: "owned",
            source: "manual_id",
            binder: None,
            deck: None,
            batch: None,
            price: Some(12.00),
            notes: Some("Foil Luke — undecided home."),
        },
        Copy {
            printing_id: "SOR-193-normal",
            condition: "Heavily Played",
            status: "owned",
            source: "manual_id",
            binder: None,
            deck: None,
            batch: None,
            price: None,
            notes: None,
        },
        // A sold copy — exercises the disposed-status path.
        Copy {
            printing_id: "SOR-262-holo",
            condition: "Near Mint",
            status: "sold",
            source: "manual_id",
            binder: None,
            deck: None,
            batch: None,
            price: Some(9.00),
            notes: Some("Sold to a friend."),
        },
    ];
    for c in &copies {
        let id = collection::add(
            conn,
            &NewCopy {
                printing_id: c.printing_id.into(),
                condition: c.condition.into_some(),
                language: None,
                purchase_price: c.price,
                acquired_at: Some("2024-01-10T12:00:00Z".into()),
                source: c.source.into(),
                status: c.status.into_some(),
                notes: c.notes.map(str::to_owned),
                order_id: None,
                binder_id: c.binder,
                deck_id: c.deck,
                batch_id: c.batch,
            },
        )?;
        // `NewCopy` has no grading fields — grade the one slabbed copy here.
        if c.notes == Some("PSA 8 slab.") {
            conn.execute(
                "UPDATE collection SET graded = 1, grade_company = 'PSA', \
                   grade_value = 8.0, grade_cert = '12345678' WHERE id = ?1",
                [id],
            )?;
        }
    }

    // --- Sealed collection --------------------------------------------
    sealed::add(
        conn,
        &NewSealed {
            product_id: 910002,
            quantity: Some(1),
            condition: "Near Mint".into_some(),
            purchase_price: Some(49.99),
            purchase_date: "2024-03-08".into_some(),
            source: "lgs".into_some(),
            seller_name: "Local Game Store".into_some(),
            notes: "Sealed Two-Player Starter — keeping factory sealed.".into_some(),
        },
    )?;
    sealed::add(
        conn,
        &NewSealed {
            product_id: 910004,
            quantity: Some(6),
            condition: "Near Mint".into_some(),
            purchase_price: Some(4.49),
            purchase_date: "2024-11-08".into_some(),
            source: "lgs".into_some(),
            seller_name: "Local Game Store".into_some(),
            notes: "Loose Shadows of the Galaxy packs to rip.".into_some(),
        },
    )?;

    // --- Wishlist ------------------------------------------------------
    wishlist::add(
        conn,
        &NewWish {
            card_id: "SOR-262".into(),
            printing_id: "SOR-262-holo".into_some(),
            max_price: Some(250.00),
            priority: Some(3),
            notes: "Hyperspace foil Luke — top of the want list.".into_some(),
            ..Default::default()
        },
    )?;
    wishlist::add(
        conn,
        &NewWish {
            card_id: "SOR-193".into(),
            printing_id: None,
            max_price: Some(120.00),
            priority: Some(2),
            notes: "Millennium Falcon for the Cunning deck.".into_some(),
            ..Default::default()
        },
    )?;
    wishlist::add(
        conn,
        &NewWish {
            card_id: "SHD-500".into(),
            printing_id: "SHD-500-normal".into_some(),
            max_price: Some(300.00),
            priority: Some(1),
            notes: "Already ordered — kept as a price watch.".into_some(),
            ..Default::default()
        },
    )?;

    Ok(())
}

/// Tiny ergonomics helper so the data tables read cleanly: `"x".into_some()`
/// instead of `Some("x".into())`.
trait IntoSome {
    fn into_some(self) -> Option<String>;
}
impl IntoSome for &str {
    fn into_some(self) -> Option<String> {
        Some(self.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::OptionalExtension;
    use std::collections::BTreeMap;
    use std::path::Path;

    /// The committed catalog fixture, addressed from the crate rather than
    /// from whatever directory the test runner happens to stand in.
    fn committed_shared() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/ui/fixtures/shared.sqlite")
    }

    /// The committed collection fixture, addressed the same way.
    fn committed_user() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/ui/fixtures/collection.sqlite")
    }

    /// The instant `tests/visual/stabilize.ts` freezes the browser at. It
    /// lives in a TypeScript file and in this Rust one, and the two cannot
    /// share a constant, so it is written down twice on purpose — see
    /// `no_fixture_row_is_dated_after_the_frozen_browser_clock`.
    const FROZEN_BROWSER_CLOCK: &str = "2026-01-15";

    /// Seed a complete fixture into a fresh directory and return both files.
    fn seed_into(dir: &Path) -> (Vec<u8>, Vec<u8>) {
        run(FixtureArgs {
            out: dir.to_path_buf(),
        })
        .expect("seed-fixture");
        (
            std::fs::read(dir.join("shared.sqlite")).unwrap(),
            std::fs::read(dir.join("collection.sqlite")).unwrap(),
        )
    }

    /// **Two regenerations of an unchanged seeder produce identical files.**
    ///
    /// The fixture is a committed BINARY artefact, so anything it records
    /// about the moment it was built is a permanent diff generator. Before
    /// this, `collection.sqlite` carried the seeding instant in eleven columns
    /// and four visual baselines — `/recent`, `/batches`, `/batches/[id]`,
    /// `/sealed`, at both viewports — had to be re-recorded every time anybody
    /// regenerated it, in the date digits and nowhere else. The cost is not
    /// the churn; it is that a real regression arrives inside the churn.
    ///
    /// Byte-identity rather than "the columns we know about", because the
    /// failure mode is a table nobody has added yet and the artefact under
    /// review is bytes. It is portable because `rusqlite` is built `bundled`,
    /// so the SQLite writing these pages is pinned by `Cargo.lock` rather than
    /// by whatever the box has installed.
    #[test]
    fn two_seed_runs_are_byte_identical() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();

        let (shared_a, user_a) = seed_into(a.path());
        // Not vacuous: two empty files compare equal.
        assert!(shared_a.len() > 100_000 && user_a.len() > 100_000);
        let (shared_b, user_b) = seed_into(b.path());

        assert!(
            shared_a == shared_b,
            "two seed-fixture runs produced different shared.sqlite \
             ({} vs {} bytes)",
            shared_a.len(),
            shared_b.len(),
        );
        assert!(
            user_a == user_b,
            "two seed-fixture runs produced different collection.sqlite \
             ({} vs {} bytes). Something on the seeding path is reading the \
             wall clock outside dumpstar_db::clock — regenerating the fixture \
             is then a diff nobody can review, and the visual baselines of \
             /recent, /batches, /batches/[id] and /sealed move with it.",
            user_a.len(),
            user_b.len(),
        );
    }

    /// Every ISO date anywhere in the committed collection — in a column or
    /// inside an outbox payload — sits before the instant the visual suite
    /// freezes the browser at.
    ///
    /// The same bug from the reader's side, and what says the COMMITTED file
    /// is the deterministic one rather than a survivor of the old behaviour: a
    /// build timestamp is always in the recent past, and the recent past is
    /// the FUTURE as far as a suite pinned to 2026-01-15 is concerned. The
    /// fixture that produced this test was dated 2026-09-04 — eight months
    /// ahead of the clock every baseline is taken under.
    ///
    /// Stated over every text cell of every table rather than over the eleven
    /// columns that were wrong: a new table with a `created_at` is exactly
    /// what nobody would think to add here.
    #[test]
    fn no_fixture_row_is_dated_after_the_frozen_browser_clock() {
        // READ-ONLY: a test that opens a committed artefact read-write is one
        // that can leave it modified, and this one is asserting about bytes.
        let conn = Connection::open_with_flags(
            committed_user(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master \
                  WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert!(!tables.is_empty(), "the committed fixture holds no tables");

        let mut seen = 0;
        let mut future = Vec::new();
        for table in &tables {
            let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
            let width = stmt.column_count();
            let names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
            let mut rows = stmt.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for (i, column) in names.iter().enumerate().take(width) {
                    let Ok(text) = row.get::<_, String>(i) else {
                        continue;
                    };
                    for date in iso_dates(&text) {
                        seen += 1;
                        if date.as_str() >= FROZEN_BROWSER_CLOCK {
                            future.push(format!("{table}.{column}: {text}"));
                        }
                    }
                }
            }
        }

        // Not vacuous: a scan that found no dates would pass whatever the
        // fixture holds.
        assert!(
            seen > 100,
            "found only {seen} dates in the committed fixture"
        );
        future.sort();
        future.dedup();
        assert!(
            future.is_empty(),
            "the committed fixture is dated after {FROZEN_BROWSER_CLOCK}, the \
             instant tests/visual/stabilize.ts freezes the browser at — which \
             is what a build timestamp looks like. Regenerate it: \
             cargo run --bin dumpstar -- seed-fixture\n  {}",
            future.join("\n  "),
        );
    }

    /// Every `YYYY-MM-DD` in `text`, wherever it sits — a bare column value,
    /// an RFC-3339 stamp, or one embedded in an outbox payload's JSON.
    fn iso_dates(text: &str) -> Vec<String> {
        let b = text.as_bytes();
        let digit = |i: usize| b[i].is_ascii_digit();
        (0..b.len().saturating_sub(9))
            .filter(|&i| {
                (0..4).all(|k| digit(i + k))
                    && b[i + 4] == b'-'
                    && digit(i + 5)
                    && digit(i + 6)
                    && b[i + 7] == b'-'
                    && digit(i + 8)
                    && digit(i + 9)
            })
            .map(|i| text[i..i + 10].to_string())
            .collect()
    }

    /// Every catalog object a database holds, mapped to its columns.
    /// `pragma_table_info` answers for views as well as tables, which is what
    /// makes `latest_sealed_prices` covered by the same pass.
    fn objects(conn: &Connection) -> rusqlite::Result<BTreeMap<String, Vec<String>>> {
        let names: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master
                  WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;

        let mut out = BTreeMap::new();
        for name in names {
            let columns: Vec<String> = conn
                .prepare("SELECT name FROM pragma_table_info(?1)")?
                .query_map([&name], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            out.insert(name, columns);
        }
        Ok(out)
    }

    /// The committed `shared.sqlite` must carry every table, view and column
    /// `schema_shared.sql` declares — because it is the ONE database this
    /// project never repairs on open. Every other file converges: the schema
    /// is re-applied with `CREATE … IF NOT EXISTS` and `add_missing_columns`
    /// ALTERs the rest. The catalog is ATTACHed READ-ONLY at request time, so
    /// a fixture generated before a schema addition stays behind forever, and
    /// the first query that names the new object dies with `no such table`.
    ///
    /// `PRAGMA user_version` cannot stand in for this check. Additive change
    /// travels by idempotent re-application and deliberately does not bump the
    /// version, so the stale fixture that produced this test reported the
    /// CURRENT version while missing two tables (pd-7hkf).
    ///
    /// Only objects the schema declares and the fixture lacks are a failure.
    /// A table the schema has since dropped, still sitting in the fixture,
    /// breaks no read — "behind" is the direction that hurts.
    #[test]
    fn the_committed_fixture_carries_every_catalog_object_the_schema_declares() {
        let dir = tempfile::tempdir().unwrap();
        let reference = dumpstar_db::open_shared(&dir.path().join("shared.sqlite")).unwrap();
        let declared = objects(&reference).unwrap();
        // Not vacuous: a reference that declared nothing would pass whatever
        // the fixture holds.
        assert!(
            !declared.is_empty(),
            "schema_shared.sql declared no objects"
        );

        let fixture = dumpstar_db::open_shared_readonly(&committed_shared()).unwrap();
        let present = objects(&fixture).unwrap();

        let mut missing = Vec::new();
        for (name, columns) in &declared {
            match present.get(name) {
                None => missing.push(name.clone()),
                Some(have) => missing.extend(
                    columns
                        .iter()
                        .filter(|c| !have.contains(c))
                        .map(|c| format!("{name}.{c}")),
                ),
            }
        }

        assert!(
            missing.is_empty(),
            "tests/ui/fixtures/shared.sqlite is behind schema_shared.sql \
             and is never repaired on open (it is ATTACHed read-only). \
             Missing: {}. Regenerate it: cargo run --bin dumpstar -- seed-fixture",
            missing.join(", "),
        );
    }

    /// The committed fixture must PRICE the sealed product it holds, and
    /// price it BOTH ways (sp-ysb).
    ///
    /// The fixture seeded four rows in `sealed_products` and none in
    /// `sealed_prices`, so both sealed lots fell to pd-bbv7's unquoted arm —
    /// skipped by the sum, counted in the units, correctly. Correct and
    /// exclusive: `/` rendered `sealed $0.00`, `/sealed` showed an em dash
    /// wherever a market value goes, and the `dimension='sealed'` series had
    /// no non-zero point on it, so no screenshot and no intent covered the
    /// join through `sealed_products` by `product_id` or either half of
    /// `sealed_market_price_expr_from!`.
    ///
    /// Stated against the COMMITTED artefact rather than against a fresh
    /// seed, because that is the file the browser tier serves: a seeder
    /// changed and a fixture not regenerated is exactly the state this bead
    /// found. It asserts the two `COALESCE` arms separately — a fixture where
    /// every quote carried a `market_price` would render identically and
    /// exercise the fallback nowhere.
    #[test]
    fn the_committed_fixture_values_its_sealed_holdings() {
        // READ-ONLY on both halves: these are committed bytes under review.
        let catalog = dumpstar_db::open_shared_readonly(&committed_shared()).unwrap();
        let user = Connection::open_with_flags(
            committed_user(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();

        // The lots the collection holds, and the units they add up to.
        let held: Vec<(i64, i64)> = user
            .prepare(
                "SELECT product_id, quantity FROM sealed_collection \
                  WHERE status = 'owned' ORDER BY product_id",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        // Not vacuous: a fixture holding no sealed product would satisfy every
        // per-lot claim below without quoting anything.
        assert!(
            held.len() >= 2,
            "the committed fixture holds {} sealed lot(s) — this test says \
             nothing without at least two, one quoted each way",
            held.len(),
        );

        // Every held product is quoted, and each quote resolves to a price
        // through the same rule the app spends.
        let mut with_market = 0;
        let mut by_mid_alone = 0;
        for (product_id, _) in &held {
            let quote: Option<(Option<f64>, Option<f64>)> = catalog
                .query_row(
                    "SELECT market_price, mid_price FROM latest_sealed_prices \
                      WHERE tcgplayer_product_id = ?1",
                    [product_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .unwrap();
            let (market, mid) = quote.unwrap_or_else(|| {
                panic!(
                    "sealed product {product_id} is held and has no row in \
                     sealed_prices, so it values at nothing. Seed one in \
                     seed_catalog and regenerate: \
                     cargo run --bin dumpstar -- seed-fixture"
                )
            });
            assert!(
                market.or(mid).is_some_and(|p| p > 0.0),
                "sealed product {product_id} is quoted a row that \
                 COALESCE(market, mid) resolves to nothing"
            );
            match market {
                Some(_) => with_market += 1,
                None => by_mid_alone += 1,
            }
        }
        assert!(
            with_market > 0,
            "no held sealed lot carries a market_price — the ordinary arm of \
             sealed_market_price_expr_from! is unexercised"
        );
        assert!(
            by_mid_alone > 0,
            "every held sealed lot carries a market_price — the COALESCE \
             fallback to mid_price is unexercised, and TCGCSV quotes plenty \
             of sealed product a mid and no market"
        );

        // And the value chart has a real sealed point on it. `card_count` is
        // UNITS on this row, not lots (pd-bbv7).
        let sealed: Option<(f64, f64, i64)> = user
            .query_row(
                "SELECT market_value, cost_basis, card_count \
                   FROM collection_value_snapshot \
                  WHERE dimension = 'sealed' ORDER BY date DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .unwrap();
        let (market_value, cost_basis, units) = sealed.expect(
            "the committed fixture has no dimension='sealed' value-history \
             row, so the sealed series on / and on the value chart is empty",
        );
        assert!(
            market_value > 0.0,
            "the sealed series is valued at {market_value} — that is the \
             unquoted arm again"
        );
        assert!(cost_basis > 0.0, "the sealed lots record what they cost");
        assert_eq!(
            units,
            held.iter().map(|(_, q)| q).sum::<i64>(),
            "card_count on the sealed row is UNITS across the held lots, \
             never the number of lots"
        );
    }
}
