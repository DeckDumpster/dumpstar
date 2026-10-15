//! `dumpstar seed-fixture` — build the deterministic Star Wars: Unlimited test
//! fixture for the intents UI-testing harness.
//!
//! Produces two SQLite files in `--out` (default `tests/ui/fixtures`):
//!
//! * `shared.sqlite` — immutable catalog (sets, cards, printings, prices,
//!   formats, sealed products): the one SWU model, `dumpstar_catalog::SCHEMA`
//!   plus dumpstar-db's overlay, as `open_shared` creates it.
//! * `collection.sqlite` — per-user mutable data (binders, decks, batches,
//!   orders, sealed, wishlist, views, collection).
//!
//! Both databases are built through Dumpstar's own code so the schema
//! init runs and the running server can open them unchanged. The intents
//! harness snapshots/restores these before each test.
//!
//! The data is small and deterministic: three real sets (names and release
//! dates), a couple of dozen cards across every card type, and printings in
//! Standard, Hyperspace, Showcase and Promo variants. Card stats and rules
//! text are paraphrases written for a fixture. The stable names are documented
//! in `tests/ui/fixtures/README.md`.

use std::path::PathBuf;

use dumpstar_core::deck::Zone;
use rusqlite::Connection;

use dumpstar_db::{
    batches::{self, NewBatch},
    binders::{self, NewBinder},
    collection::{self, NewCopy},
    decks::{self, NewDeck},
    orders::{self, NewOrder, OrderLine},
    sealed::{self, NewSealed},
    user_printings::{self, NewUserPrinting},
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
/// the day its prices were quoted; it sits after the newest set's release
/// (Jump to Lightspeed, 2025-03-14), after the last order date the fixture
/// invents (2025-06-14) and after the acquisition constant on the
/// manually-entered copies (`ACQUIRED_AT`), so the collection reads as one that
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
const SEEDED_AT: &str = "2025-06-15T09:00:00Z";
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

// ---------------------------------------------------------------------------
// Catalog (shared.sqlite)
// ---------------------------------------------------------------------------

/// The fixture's "today": the day the newest prices were quoted. Fixed, so
/// prices are byte-for-byte reproducible.
const OBSERVED_AT: &str = "2025-06-15";
/// The day before, and the day before that: the older quotes some printings
/// carry, so a price has a movement and the value history has more than one
/// day to reconstruct.
const OBSERVED_PREV: &str = "2025-06-14";
const OBSERVED_EARLIER: &str = "2025-06-13";
/// When the fixture's sealed products were catalogued.
const FETCHED_AT: &str = "2025-06-15T00:00:00Z";
/// When the fixture's own copies were acquired.
const ACQUIRED_AT: &str = "2025-06-10T12:00:00Z";

/// A set: code, name, release date, sort order. The dates are the real ones.
type SetRow = (&'static str, &'static str, &'static str, i64);

const SETS: [SetRow; 3] = [
    ("SOR", "Spark of Rebellion", "2024-03-08", 1),
    ("SHD", "Shadows of the Galaxy", "2024-07-12", 2),
    ("JTL", "Jump to Lightspeed", "2025-03-14", 3),
];

/// A game card: everything its rules text says, shared by all its printings.
struct CardRow {
    id: &'static str,
    name: &'static str,
    subtitle: Option<&'static str>,
    card_type: &'static str,
    cost: Option<i64>,
    power: Option<i64>,
    hp: Option<i64>,
    aspects: &'static [&'static str],
    traits: &'static [&'static str],
    arenas: &'static [&'static str],
    rules: &'static str,
}

/// Shorthand for the table below: `(id, name, subtitle, type)` then the
/// numbers `(cost, power, hp)` then `(aspects, traits, arenas)` and the text.
#[allow(clippy::too_many_arguments)]
const fn card(
    id: &'static str,
    name: &'static str,
    subtitle: Option<&'static str>,
    card_type: &'static str,
    stats: (Option<i64>, Option<i64>, Option<i64>),
    aspects: &'static [&'static str],
    traits: &'static [&'static str],
    arenas: &'static [&'static str],
    rules: &'static str,
) -> CardRow {
    CardRow {
        id,
        name,
        subtitle,
        card_type,
        cost: stats.0,
        power: stats.1,
        hp: stats.2,
        aspects,
        traits,
        arenas,
        rules,
    }
}

/// The fixture's cards. The names are real; the stats and rules text are
/// close paraphrases written for a test fixture, not a card database.
const CARDS: [CardRow; 26] = [
    card(
        "SOR_005",
        "Luke Skywalker",
        Some("Faithful Friend"),
        "Leader",
        (Some(6), Some(4), Some(7)),
        &["Vigilance", "Heroism"],
        &["Force", "Rebel"],
        &["Ground"],
        "On Attack: Give a Shield token to a unit.",
    ),
    card(
        "SOR_010",
        "Darth Vader",
        Some("Dark Lord of the Sith"),
        "Leader",
        (Some(7), Some(5), Some(8)),
        &["Aggression", "Villainy"],
        &["Imperial", "Force", "Sith"],
        &["Ground"],
        "On Attack: You may deal 1 damage to a friendly unit. If you do, deal 2 damage to an enemy unit.",
    ),
    card(
        "SOR_020",
        "Echo Base",
        None,
        "Base",
        (None, None, Some(30)),
        &["Heroism"],
        &["Rebel"],
        &["Ground"],
        "Epic Action: Play a Heroism unit from your hand. It costs 2 less.",
    ),
    card(
        "SOR_024",
        "Dagobah Swamp",
        None,
        "Base",
        (None, None, Some(28)),
        &["Vigilance"],
        &[],
        &["Ground"],
        "Epic Action: Heal 2 damage from a unit.",
    ),
    card(
        "SOR_193",
        "Millennium Falcon",
        Some("Piece of Junk"),
        "Unit",
        (Some(3), Some(3), Some(4)),
        &["Cunning", "Heroism"],
        &["Vehicle", "Transport", "Underworld"],
        &["Space"],
        "When Played: You may play a Rebel unit from your hand.",
    ),
    card(
        "SOR_070",
        "Battlefield Marine",
        None,
        "Unit",
        (Some(2), Some(3), Some(3)),
        &["Command", "Heroism"],
        &["Rebel", "Trooper"],
        &["Ground"],
        "",
    ),
    card(
        "SOR_085",
        "Wampa",
        None,
        "Unit",
        (Some(4), Some(4), Some(5)),
        &["Aggression"],
        &["Creature"],
        &["Ground"],
        "",
    ),
    card(
        "SOR_100",
        "Death Star Stormtrooper",
        None,
        "Unit",
        (Some(1), Some(3), Some(1)),
        &["Aggression", "Villainy"],
        &["Imperial", "Trooper"],
        &["Ground"],
        "",
    ),
    card(
        "SOR_120",
        "Cartel Spacer",
        None,
        "Unit",
        (Some(2), Some(2), Some(3)),
        &["Cunning"],
        &["Underworld", "Transport"],
        &["Space"],
        "When Played: If you control another Cunning unit, exhaust an enemy unit.",
    ),
    card(
        "SOR_130",
        "TIE/ln Fighter",
        None,
        "Unit",
        (Some(1), Some(2), Some(1)),
        &["Aggression", "Villainy"],
        &["Imperial", "Vehicle", "Fighter"],
        &["Space"],
        "",
    ),
    card(
        "SOR_140",
        "Snowtrooper Lieutenant",
        None,
        "Unit",
        (Some(3), Some(3), Some(3)),
        &["Aggression", "Villainy"],
        &["Imperial", "Trooper"],
        &["Ground"],
        "On Attack: You may attack with another Imperial unit.",
    ),
    card(
        "SOR_150",
        "Fleet Lieutenant",
        None,
        "Unit",
        (Some(3), Some(3), Some(3)),
        &["Aggression", "Heroism"],
        &["Rebel", "Official"],
        &["Ground"],
        "When Played: You may attack with a Rebel unit.",
    ),
    card(
        "SOR_160",
        "Vigilant Honor Guards",
        None,
        "Unit",
        (Some(4), Some(3), Some(6)),
        &["Vigilance", "Villainy"],
        &["Imperial", "Trooper"],
        &["Ground"],
        "This unit cannot be attacked while it is undamaged.",
    ),
    card(
        "SOR_170",
        "Vanquish",
        None,
        "Event",
        (Some(5), None, None),
        &["Command"],
        &[],
        &[],
        "Defeat a non-leader unit.",
    ),
    card(
        "SOR_171",
        "Overwhelming Barrage",
        None,
        "Event",
        (Some(4), None, None),
        &["Aggression"],
        &[],
        &[],
        "Give a friendly unit +2/+2 for this phase. Then distribute damage equal to its power among enemy units.",
    ),
    card(
        "SOR_172",
        "Power of the Dark Side",
        None,
        "Event",
        (Some(6), None, None),
        &["Aggression", "Villainy"],
        &["Force"],
        &[],
        "An opponent chooses a unit they control. Defeat it.",
    ),
    card(
        "SOR_173",
        "Waylay",
        None,
        "Event",
        (Some(3), None, None),
        &["Cunning"],
        &[],
        &[],
        "Return a non-leader unit to its owner's hand.",
    ),
    card(
        "SOR_174",
        "Repair",
        None,
        "Event",
        (Some(1), None, None),
        &["Vigilance"],
        &[],
        &[],
        "Heal 3 damage from a unit or base.",
    ),
    card(
        "SOR_180",
        "Entrenched",
        None,
        "Upgrade",
        (Some(2), Some(2), Some(2)),
        &["Vigilance"],
        &[],
        &[],
        "Attached unit cannot be defeated by enemy card effects.",
    ),
    card(
        "SOR_181",
        "Fallen Lightsaber",
        None,
        "Upgrade",
        (Some(2), Some(3), Some(1)),
        &["Aggression"],
        &["Item", "Weapon", "Lightsaber"],
        &[],
        "Attached unit gains: On Attack: Deal 1 damage to each base.",
    ),
    card(
        "SOR_182",
        "Jedi Lightsaber",
        None,
        "Upgrade",
        (Some(3), Some(3), Some(1)),
        &["Command"],
        &["Item", "Weapon", "Lightsaber"],
        &[],
        "Attach to a non-Vehicle unit. If attached unit is a Force unit, it gets +1/+1 more.",
    ),
    card(
        "SHD_001",
        "Cad Bane",
        Some("Now It's Personal"),
        "Leader",
        (Some(5), Some(3), Some(6)),
        &["Cunning", "Villainy"],
        &["Underworld", "Bounty Hunter"],
        &["Ground"],
        "Action: Exhaust this leader. Deal 1 damage to a unit.",
    ),
    card(
        "SHD_020",
        "Level 1313",
        None,
        "Base",
        (None, None, Some(28)),
        &["Cunning"],
        &["Underworld"],
        &["Ground"],
        "Epic Action: Give an Experience token to a unit.",
    ),
    card(
        "SHD_100",
        "Bossk",
        Some("Deadly Stalker"),
        "Unit",
        (Some(4), Some(5), Some(5)),
        &["Aggression", "Villainy"],
        &["Trandoshan", "Bounty Hunter"],
        &["Ground"],
        "Bounty: Deal 2 damage to a base.",
    ),
    card(
        "JTL_001",
        "Wedge Antilles",
        Some("Leader of Red Squadron"),
        "Leader",
        (Some(5), Some(3), Some(6)),
        &["Heroism", "Command"],
        &["Rebel", "Pilot"],
        &["Space"],
        "Action: Exhaust this leader. Attach a Pilot upgrade from your hand.",
    ),
    card(
        "JTL_050",
        "X-Wing",
        None,
        "Unit",
        (Some(3), Some(3), Some(4)),
        &["Vigilance", "Heroism"],
        &["Rebel", "Vehicle", "Fighter"],
        &["Space"],
        "Piloting [2 Resources]",
    ),
];

/// One printing of a card, with the fixture's market quotes for its two
/// finishes. A `None` quote leaves that finish deliberately unpriced.
struct PrintingRow {
    card_id: &'static str,
    set: &'static str,
    number: &'static str,
    variant: &'static str,
    rarity: &'static str,
    artist: &'static str,
    normal: Option<f64>,
    foil: Option<f64>,
}

const fn printing(
    card_id: &'static str,
    set: &'static str,
    number: &'static str,
    variant: &'static str,
    rarity: &'static str,
    artist: &'static str,
    (normal, foil): (Option<f64>, Option<f64>),
) -> PrintingRow {
    PrintingRow {
        card_id,
        set,
        number,
        variant,
        rarity,
        artist,
        normal,
        foil,
    }
}

/// Every printing gets a Normal and a Foil row in `printings`; the quotes
/// decide which of them also get `prices`. Hyperspace and Showcase printings
/// have their own collector numbers, as in the real sets.
const PRINTINGS: [PrintingRow; 43] = [
    printing(
        "SOR_005",
        "SOR",
        "005",
        "Standard",
        "Special",
        "Borja Pindado",
        (Some(3.50), Some(14.00)),
    ),
    printing(
        "SOR_005",
        "SOR",
        "262",
        "Hyperspace",
        "Special",
        "Borja Pindado",
        (Some(9.00), Some(30.00)),
    ),
    printing(
        "SOR_005",
        "SOR",
        "520",
        "Showcase",
        "Special",
        "Borja Pindado",
        (Some(85.00), None),
    ),
    printing(
        "SOR_010",
        "SOR",
        "010",
        "Standard",
        "Rare",
        "Matt Ferguson",
        (Some(2.75), Some(11.50)),
    ),
    printing(
        "SOR_010",
        "SOR",
        "263",
        "Hyperspace",
        "Rare",
        "Matt Ferguson",
        (Some(7.80), Some(26.00)),
    ),
    printing(
        "SOR_020",
        "SOR",
        "020",
        "Standard",
        "Common",
        "Aaron Hwang",
        (Some(0.12), Some(0.60)),
    ),
    printing(
        "SOR_024",
        "SOR",
        "024",
        "Standard",
        "Common",
        "Aaron Hwang",
        (Some(0.10), Some(0.55)),
    ),
    printing(
        "SOR_193",
        "SOR",
        "193",
        "Standard",
        "Legendary",
        "Kuroimori",
        (Some(6.40), Some(21.00)),
    ),
    printing(
        "SOR_193",
        "SOR",
        "264",
        "Hyperspace",
        "Legendary",
        "Kuroimori",
        (Some(18.50), Some(55.00)),
    ),
    printing(
        "SOR_193",
        "SOR",
        "521",
        "Showcase",
        "Legendary",
        "Kuroimori",
        (Some(140.00), None),
    ),
    printing(
        "SOR_070",
        "SOR",
        "070",
        "Standard",
        "Common",
        "Jake Murray",
        (Some(0.08), Some(0.45)),
    ),
    printing(
        "SOR_085",
        "SOR",
        "085",
        "Standard",
        "Common",
        "Jake Murray",
        (Some(0.10), Some(0.40)),
    ),
    printing(
        "SOR_100",
        "SOR",
        "100",
        "Standard",
        "Common",
        "Bruno Werneck",
        (Some(0.07), None),
    ),
    printing(
        "SOR_120",
        "SOR",
        "120",
        "Standard",
        "Common",
        "Bruno Werneck",
        (Some(0.09), Some(0.50)),
    ),
    printing(
        "SOR_130",
        "SOR",
        "130",
        "Standard",
        "Uncommon",
        "Brian Valenzuela",
        (Some(0.20), Some(0.95)),
    ),
    printing(
        "SOR_140",
        "SOR",
        "140",
        "Standard",
        "Uncommon",
        "Brian Valenzuela",
        (Some(0.18), Some(0.85)),
    ),
    printing(
        "SOR_150",
        "SOR",
        "150",
        "Standard",
        "Uncommon",
        "Matt Ferguson",
        (Some(0.22), Some(1.10)),
    ),
    printing(
        "SOR_150",
        "SOR",
        "266",
        "Hyperspace",
        "Uncommon",
        "Matt Ferguson",
        (Some(0.90), Some(3.60)),
    ),
    printing(
        "SOR_160",
        "SOR",
        "160",
        "Standard",
        "Uncommon",
        "Aaron Hwang",
        (Some(0.15), Some(0.70)),
    ),
    printing(
        "SOR_170",
        "SOR",
        "170",
        "Standard",
        "Rare",
        "Kuroimori",
        (Some(1.10), Some(4.80)),
    ),
    printing(
        "SOR_170",
        "SOR",
        "267",
        "Hyperspace",
        "Rare",
        "Kuroimori",
        (Some(3.20), Some(12.00)),
    ),
    printing(
        "SOR_171",
        "SOR",
        "171",
        "Standard",
        "Uncommon",
        "Jake Murray",
        (Some(0.25), Some(1.20)),
    ),
    printing(
        "SOR_172",
        "SOR",
        "172",
        "Standard",
        "Rare",
        "Bruno Werneck",
        (Some(0.90), Some(3.90)),
    ),
    printing(
        "SOR_172",
        "SOR",
        "268",
        "Hyperspace",
        "Rare",
        "Bruno Werneck",
        (None, None),
    ),
    printing(
        "SOR_173",
        "SOR",
        "173",
        "Standard",
        "Uncommon",
        "Aaron Hwang",
        (Some(0.20), Some(0.90)),
    ),
    printing(
        "SOR_174",
        "SOR",
        "174",
        "Standard",
        "Common",
        "Jake Murray",
        (Some(0.06), Some(0.35)),
    ),
    printing(
        "SOR_180",
        "SOR",
        "180",
        "Standard",
        "Common",
        "Brian Valenzuela",
        (Some(0.12), Some(0.55)),
    ),
    printing(
        "SOR_181",
        "SOR",
        "181",
        "Standard",
        "Uncommon",
        "Kuroimori",
        (Some(0.35), Some(1.40)),
    ),
    printing(
        "SOR_182",
        "SOR",
        "182",
        "Standard",
        "Rare",
        "Kuroimori",
        (Some(1.30), Some(5.20)),
    ),
    printing(
        "SOR_182",
        "SOR",
        "269",
        "Hyperspace",
        "Rare",
        "Kuroimori",
        (None, None),
    ),
    printing(
        "SHD_001",
        "SHD",
        "001",
        "Standard",
        "Rare",
        "Matt Ferguson",
        (Some(2.40), Some(9.50)),
    ),
    printing(
        "SHD_001",
        "SHD",
        "262",
        "Hyperspace",
        "Rare",
        "Matt Ferguson",
        (Some(6.10), Some(20.00)),
    ),
    printing(
        "SHD_001",
        "SHD",
        "515",
        "Showcase",
        "Special",
        "Matt Ferguson",
        (Some(45.00), None),
    ),
    printing(
        "SHD_020",
        "SHD",
        "020",
        "Standard",
        "Common",
        "Aaron Hwang",
        (Some(0.11), Some(0.60)),
    ),
    printing(
        "SHD_100",
        "SHD",
        "100",
        "Standard",
        "Legendary",
        "Bruno Werneck",
        (Some(4.90), Some(16.00)),
    ),
    printing(
        "JTL_001",
        "JTL",
        "001",
        "Standard",
        "Rare",
        "Jake Murray",
        (Some(1.60), Some(6.40)),
    ),
    printing(
        "JTL_050",
        "JTL",
        "050",
        "Standard",
        "Uncommon",
        "Brian Valenzuela",
        (Some(0.30), Some(1.30)),
    ),
    printing(
        "JTL_050",
        "JTL",
        "265",
        "Hyperspace",
        "Uncommon",
        "Brian Valenzuela",
        (Some(1.10), Some(4.20)),
    ),
    // Promos: their own variant, numbered outside the set's run.
    printing(
        "SOR_170",
        "SOR",
        "P01",
        "Promo",
        "Special",
        "Kuroimori",
        (Some(2.20), Some(7.00)),
    ),
    printing(
        "SOR_193",
        "SOR",
        "P02",
        "Promo",
        "Special",
        "Kuroimori",
        (Some(11.00), None),
    ),
    printing(
        "SOR_005",
        "SOR",
        "P03",
        "Promo",
        "Special",
        "Borja Pindado",
        (None, Some(40.00)),
    ),
    printing(
        "SHD_100",
        "SHD",
        "P04",
        "Promo",
        "Special",
        "Bruno Werneck",
        (Some(3.30), Some(9.90)),
    ),
    printing(
        "JTL_001",
        "JTL",
        "P05",
        "Promo",
        "Special",
        "Jake Murray",
        (None, None),
    ),
];

/// The printing id of `number` in `set`, for a finish: `<official uid>:<Finish>`,
/// where the uid is `<SET>-<number>`. Same shape as production's
/// `<cardUid>:<Finish>`.
fn printing_id(set: &str, number: &str, finish: &str) -> String {
    format!("{set}-{number}:{finish}")
}

/// A sealed product: TCGplayer product id, set, name, subtype, release date.
type SealedRow = (i64, &'static str, &'static str, &'static str, &'static str);

const SEALED: [SealedRow; 4] = [
    (
        900001,
        "SOR",
        "Spark of Rebellion Booster Box",
        "Booster Box",
        "2024-03-08",
    ),
    (
        900002,
        "SHD",
        "Shadows of the Galaxy Booster Box",
        "Booster Box",
        "2024-07-12",
    ),
    (
        900003,
        "SOR",
        "Spark of Rebellion Two-Player Starter",
        "Starter",
        "2024-03-08",
    ),
    (
        900004,
        "JTL",
        "Jump to Lightspeed Booster Pack",
        "Booster Pack",
        "2025-03-14",
    ),
];

/// A sealed-price row: product id, observed on, low, mid, high, market,
/// direct low.
///
/// `market` is `Option` because TCGCSV genuinely quotes many sealed products a
/// mid and no market, and `sealed_market_price_expr_from!` exists to pick
/// between the two off ONE observation. A fixture that filled `market` on
/// every row would leave that `COALESCE` unexercised by anything a screenshot
/// can see.
type SealedPriceRow = (i64, &'static str, f64, f64, f64, Option<f64>, Option<f64>);

/// The two products the collection actually HOLDS, quoted one way each, so
/// the rendered sealed value goes through the whole rule rather than one arm
/// of it (pd-bbv7, sp-ysb):
///
///   900002  a full quote, `market_price` set — the ordinary arm (and quoted
///           on two days, so the newest stands)
///   900004  `market_price` NULL, mid alone — the `COALESCE` fallback
///
/// 900001 and 900003 stay unquoted deliberately. Neither is held, so neither
/// moves a number, and leaving them keeps the catalog carrying the shape
/// TCGCSV really produces: a sealed product with no price row at all.
///
/// The two owned lots then value at 96.40 + 6 x 5.24 = 127.84 against the
/// 116.93 they cost, so the sealed series reads as a small gain rather than as
/// a round number nobody would notice was wrong.
const SEALED_PRICES: [SealedPriceRow; 3] = [
    (
        900002,
        OBSERVED_PREV,
        86.00,
        93.10,
        115.00,
        Some(94.80),
        Some(90.00),
    ),
    (
        900002,
        OBSERVED_AT,
        88.00,
        94.75,
        119.99,
        Some(96.40),
        Some(91.00),
    ),
    (900004, OBSERVED_AT, 4.10, 5.24, 8.99, None, None),
];

fn r2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn seed_catalog(conn: &Connection) -> anyhow::Result<()> {
    for (code, name, release, sort) in SETS {
        conn.execute(
            "INSERT INTO sets (set_code, name, release_date, sort_order) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![code, name, release, sort],
        )?;
    }

    for c in &CARDS {
        // None of these values contains a quote or a backslash, so a JSON array
        // is the quoted names joined; no serializer needed.
        let json = |v: &[&str]| -> String {
            format!(
                "[{}]",
                v.iter()
                    .map(|s| format!("\"{s}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        conn.execute(
            "INSERT INTO cards \
               (card_id, name, subtitle, card_type, cost, power, hp, aspects, \
                traits, arenas, rules_text) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            rusqlite::params![
                c.id,
                c.name,
                c.subtitle,
                c.card_type,
                c.cost,
                c.power,
                c.hp,
                json(c.aspects),
                json(c.traits),
                json(c.arenas),
                c.rules,
            ],
        )?;
        // The full-text row, exactly as the catalog apply job writes it.
        conn.execute(
            "INSERT INTO card_search (card_id, name, subtitle, rules_text, traits) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![c.id, c.name, c.subtitle, c.rules, c.traits.join(" ")],
        )?;
    }

    // Printings, their prices and the crosswalk. A tcgcsv product id is
    // allocated per row that has at least one quote, and shared by the
    // printing's two finishes, as TCGplayer shares it.
    for (i, p) in PRINTINGS.iter().enumerate() {
        let uid = format!("{}-{}", p.set, p.number);
        let product_id = 700_000 + i as i64;
        for (finish, quote) in [("Normal", p.normal), ("Foil", p.foil)] {
            let id = printing_id(p.set, p.number, finish);
            conn.execute(
                "INSERT INTO printings \
                   (printing_id, card_id, set_code, number, variant, finish, \
                    rarity, artist, image_url) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
                rusqlite::params![
                    id, p.card_id, p.set, p.number, p.variant, finish, p.rarity, p.artist
                ],
            )?;
            conn.execute(
                "INSERT INTO printing_source_id (source, source_id, sub_type_name, printing_id) \
                 VALUES ('official', ?1, ?2, ?3)",
                rusqlite::params![uid, finish, id],
            )?;
            let Some(market) = quote else { continue };
            conn.execute(
                "INSERT INTO printing_source_id (source, source_id, sub_type_name, printing_id) \
                 VALUES ('tcgcsv', ?1, ?2, ?3)",
                rusqlite::params![product_id.to_string(), finish, id],
            )?;
            // Newest quote on every priced printing; every third one also has
            // yesterday's, every seventh the day before that, so some prices
            // have moved and some have not.
            let mut days = vec![(OBSERVED_AT, 1.0)];
            if i % 3 == 0 {
                days.push((OBSERVED_PREV, 0.95));
            }
            if i % 7 == 0 {
                days.push((OBSERVED_EARLIER, 0.9));
            }
            for (day, scale) in days {
                let m = r2(market * scale);
                let direct_low = (m >= 5.0).then(|| r2(m * 0.8));
                conn.execute(
                    "INSERT INTO prices \
                       (printing_id, source, observed_on, low, mid, high, market, direct_low) \
                     VALUES (?1, 'tcgcsv', ?2, ?3, ?4, ?5, ?6, ?7)",
                    rusqlite::params![
                        id,
                        day,
                        r2(m * 0.75),
                        r2(m * 0.95),
                        r2(m * 1.4),
                        m,
                        direct_low
                    ],
                )?;
            }
        }
    }

    // Formats come from the catalog crate's own data, so the fixture's rules
    // are the real ones; a few legality rows are the fixture's invention, to
    // give the legality checker something to say.
    for f in dumpstar_catalog::formats::builtin_formats() {
        conn.execute(
            "INSERT INTO formats (format_id, name) VALUES (?1, ?2)",
            rusqlite::params![f.format_id, f.name],
        )?;
        for (key, value) in &f.rules {
            conn.execute(
                "INSERT INTO format_rules (format_id, rule_key, rule_value) VALUES (?1, ?2, ?3)",
                rusqlite::params![f.format_id, key, value],
            )?;
        }
    }
    for (format, card, status) in [
        ("premier", "SOR_193", "banned"),
        ("premier", "SOR_172", "restricted"),
        ("twin_suns", "SOR_170", "not_legal"),
    ] {
        conn.execute(
            "INSERT INTO card_legality (format_id, card_id, status) VALUES (?1, ?2, ?3)",
            rusqlite::params![format, card, status],
        )?;
    }

    for (pid, set, name, subtype, release) in SEALED {
        conn.execute(
            "INSERT INTO sealed_products \
               (product_id, set_code, name, category, subtype, release_date, \
                image_url, tcgplayer_url, fetched_at) \
             VALUES (?1, ?2, ?3, 'sealed', ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                pid,
                set,
                name,
                subtype,
                release,
                format!("https://product-images.tcgplayer.com/{pid}.jpg"),
                format!("https://www.tcgplayer.com/product/{pid}"),
                FETCHED_AT,
            ],
        )?;
    }
    for (pid, day, low, mid, high, market, direct_low) in SEALED_PRICES {
        conn.execute(
            "INSERT INTO sealed_prices \
               (tcgplayer_product_id, low_price, mid_price, high_price, \
                market_price, direct_low_price, observed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![pid, low, mid, high, market, direct_low, day],
        )?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// User data (collection.sqlite)
// ---------------------------------------------------------------------------

fn catalog_counts(conn: &Connection) -> anyhow::Result<(i64, i64, i64, i64)> {
    Ok((
        conn.query_row("SELECT count(*) FROM sets", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM cards", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM printings", [], |r| r.get(0))?,
        conn.query_row("SELECT count(*) FROM prices", [], |r| r.get(0))?,
    ))
}

// ---------------------------------------------------------------------------
// User data (collection.sqlite)
// ---------------------------------------------------------------------------

/// The main deck of "Luke Vigilance Midrange": 18 distinct cards, 50 in all,
/// within the three-copy cap — a legal Premier deck with its leader and base.
const LUKE_MAIN: [(&str, u32); 18] = [
    ("SOR_070", 3),
    ("SOR_085", 3),
    ("SOR_100", 3),
    ("SOR_120", 3),
    ("SOR_130", 3),
    ("SOR_140", 3),
    ("SOR_150", 3),
    ("SOR_160", 3),
    ("SOR_170", 3),
    ("SOR_171", 3),
    ("SOR_173", 3),
    ("SOR_174", 3),
    ("SOR_180", 3),
    ("SOR_181", 3),
    ("SOR_182", 3),
    ("SHD_100", 3),
    ("JTL_050", 1),
    ("SOR_172", 1),
];

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
            description: Some("Working toward a complete Spark of Rebellion set.".into()),
            color: "blue".into_some(),
            binder_type: "set".into_some(),
            pocket_size: Some(12),
            storage_location: "Shelf A".into_some(),
        },
    )?;
    let vault = binders::create(
        conn,
        &NewBinder {
            name: "Hyperspace Vault".into(),
            description: Some("High-value Hyperspace and Showcase printings.".into()),
            color: "gold".into_some(),
            binder_type: "showcase".into_some(),
            pocket_size: Some(9),
            storage_location: "Safe".into_some(),
        },
    )?;

    // --- Decks (idea / ready / built) ----------------------------------
    // Each deck's lines are game cards (`card_id`), never printings; the
    // built deck is a legal Premier shape: one Leader, one Base, 50 main.
    let luke_deck = decks::create(
        conn,
        &NewDeck {
            name: "Luke Vigilance Midrange".into(),
            description: Some("Vigilance and Heroism midrange around Luke.".into()),
            format: "premier".into_some(),
            owner: "Ryan".into_some(),
            state: "built".into_some(),
            sleeve_color: "black".into_some(),
            storage_location: "Deck Box 1".into_some(),
            notes: "Tournament-ready.".into_some(),
        },
    )?;
    decks::set_card(conn, luke_deck, "SOR_005", Zone::Main, 1)?;
    decks::set_card(conn, luke_deck, "SOR_024", Zone::Main, 1)?;
    for (card_id, qty) in LUKE_MAIN {
        decks::set_card(conn, luke_deck, card_id, Zone::Main, qty)?;
    }

    let vader_deck = decks::create(
        conn,
        &NewDeck {
            name: "Vader Aggro".into(),
            description: Some("Fast Aggression beats.".into()),
            format: "premier".into_some(),
            owner: "Ryan".into_some(),
            state: "ready".into_some(),
            sleeve_color: "red".into_some(),
            storage_location: "Deck Box 2".into_some(),
            notes: None,
        },
    )?;
    decks::set_card(conn, vader_deck, "SOR_010", Zone::Main, 1)?;
    decks::set_card(conn, vader_deck, "SOR_020", Zone::Main, 1)?;
    for card_id in ["SOR_100", "SOR_130", "SOR_140", "SOR_171"] {
        decks::set_card(conn, vader_deck, card_id, Zone::Main, 3)?;
    }
    let bounty_deck = decks::create(
        conn,
        &NewDeck {
            name: "Bounty Hunters".into(),
            description: Some("A casual deck idea around Cad Bane.".into()),
            format: "premier".into_some(),
            owner: "Alice".into_some(),
            state: "idea".into_some(),
            sleeve_color: None,
            storage_location: None,
            notes: "Needs a lot more bounties.".into_some(),
        },
    )?;
    decks::set_card(conn, bounty_deck, "SHD_001", Zone::Main, 1)?;
    decks::set_card(conn, bounty_deck, "SHD_100", Zone::Main, 3)?;

    // --- Orders --------------------------------------------------------
    // One received order and one still-open order. `orders::create` seeds
    // its own batch + `ordered` collection rows; `receive` flips them owned.
    let received_order = orders::create(
        conn,
        &NewOrder {
            order_number: "TCG-100001".into_some(),
            source: "tcgplayer".into(),
            seller_name: "CardKingdomLA".into_some(),
            order_date: "2025-06-05".into_some(),
            subtotal: Some(4.75),
            shipping: Some(4.99),
            tax: Some(0.00),
            total: Some(9.74),
            shipping_status: "delivered".into_some(),
            estimated_delivery: "2025-06-09".into_some(),
            notes: "Singles for the Luke deck.".into_some(),
        },
        &[
            OrderLine {
                printing_id: printing_id("SOR", "070", "Normal"),
                quantity: 3,
                purchase_price: Some(0.25),
            },
            OrderLine {
                printing_id: printing_id("SOR", "170", "Foil"),
                quantity: 1,
                purchase_price: Some(4.00),
            },
        ],
    )?;
    orders::receive(conn, received_order)?;

    let _open_order = orders::create(
        conn,
        &NewOrder {
            order_number: "EBAY-55012".into_some(),
            source: "ebay".into(),
            seller_name: "hyperspace_pulls".into_some(),
            order_date: "2025-06-14".into_some(),
            subtotal: Some(75.00),
            shipping: Some(0.00),
            tax: Some(0.00),
            total: Some(75.00),
            shipping_status: "shipped".into_some(),
            estimated_delivery: "2025-06-22".into_some(),
            notes: "Showcase Luke — still in transit.".into_some(),
        },
        &[OrderLine {
            printing_id: printing_id("SOR", "520", "Normal"),
            quantity: 1,
            purchase_price: Some(75.00),
        }],
    )?;

    // --- Batches -------------------------------------------------------
    // (Orders already created their own batches; these are the manual /
    // binder-click / CSV ingest batches.)
    let manual_batch = batches::create(
        conn,
        &NewBatch {
            batch_type: "manual_id".into(),
            name: "Hyperspace entry".into_some(),
            notes: "Hand-entered Hyperspace and Showcase cards.".into_some(),
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
            name: "CSV export 2025-06".into_some(),
            notes: "Bulk import from a CSV.".into_some(),
            order_id: None,
            binder_id: Some(trade_binder),
        },
    )?;

    // --- A user printing -----------------------------------------------
    // A variant the catalog does not model: the escape hatch for a misprint.
    let misprint = user_printings::insert(
        conn,
        &NewUserPrinting {
            card_id: "SOR_193".into(),
            description: Some("Misprint — missing foil stamp.".into()),
        },
    )?;

    // `created_at` is a column default, so it reads SQLite's clock rather than
    // the pinned one; date it from the fixture's own timeline.
    conn.execute(
        "UPDATE user_printings SET created_at = ?1 WHERE printing_id = ?2",
        rusqlite::params![ACQUIRED_AT, misprint.printing_id],
    )?;

    // --- Collection copies --------------------------------------------
    struct Copy {
        printing_id: String,
        condition: &'static str,
        status: &'static str,
        source: &'static str,
        binder: Option<i64>,
        deck: Option<i64>,
        batch: Option<i64>,
        price: Option<f64>,
        notes: Option<&'static str>,
    }
    // A copy with the common defaults; the table below overrides the rest.
    let copy = |set: &str, number: &str, finish: &str, condition: &'static str| Copy {
        printing_id: printing_id(set, number, finish),
        condition,
        status: "owned",
        source: "manual_id",
        binder: None,
        deck: None,
        batch: None,
        price: None,
        notes: None,
    };
    let copies = vec![
        // Hyperspace Vault — manual batch.
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(28.00),
            notes: Some("Hyperspace Luke — foil."),
            ..copy("SOR", "262", "Foil", "Near Mint")
        },
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(16.00),
            ..copy("SOR", "264", "Normal", "Lightly Played")
        },
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(120.00),
            notes: Some("Showcase Falcon — the chase card."),
            ..copy("SOR", "521", "Normal", "Near Mint")
        },
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(40.00),
            ..copy("SHD", "515", "Normal", "Near Mint")
        },
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(6.50),
            ..copy("SOR", "263", "Normal", "Near Mint")
        },
        // A graded card (set via direct UPDATE after insert).
        Copy {
            binder: Some(vault),
            batch: Some(manual_batch),
            price: Some(70.00),
            notes: Some("PSA 9 slab."),
            ..copy("SOR", "520", "Normal", "Near Mint")
        },
        // Spark of Rebellion binder-click registrations — Master Set binder.
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            ..copy("SOR", "005", "Normal", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            ..copy("SOR", "005", "Foil", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            ..copy("SOR", "010", "Normal", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            ..copy("SOR", "010", "Foil", "Lightly Played")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            ..copy("SOR", "020", "Normal", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            price: Some(0.10),
            ..copy("SOR", "024", "Normal", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            price: Some(5.50),
            ..copy("SOR", "193", "Normal", "Near Mint")
        },
        Copy {
            source: "binder_click",
            binder: Some(masters),
            batch: Some(click_batch),
            price: Some(3.00),
            notes: Some("Foil Vanquish."),
            ..copy("SOR", "170", "Foil", "Near Mint")
        },
        // CSV import — Trade Binder.
        Copy {
            source: "csv_manabox",
            binder: Some(trade_binder),
            batch: Some(csv_batch),
            ..copy("SHD", "001", "Normal", "Near Mint")
        },
        Copy {
            source: "csv_manabox",
            binder: Some(trade_binder),
            batch: Some(csv_batch),
            ..copy("SHD", "020", "Normal", "Near Mint")
        },
        Copy {
            source: "csv_manabox",
            binder: Some(trade_binder),
            batch: Some(csv_batch),
            ..copy("JTL", "050", "Normal", "Lightly Played")
        },
        Copy {
            status: "listed",
            source: "csv_manabox",
            binder: Some(trade_binder),
            batch: Some(csv_batch),
            price: Some(5.00),
            notes: Some("Listed for trade — Wedge."),
            ..copy("JTL", "001", "Foil", "Near Mint")
        },
        // Cards built into the Luke Vigilance Midrange deck.
        Copy {
            deck: Some(luke_deck),
            price: Some(3.00),
            notes: Some("Deck leader."),
            ..copy("SOR", "005", "Normal", "Near Mint")
        },
        Copy {
            deck: Some(luke_deck),
            price: Some(0.10),
            ..copy("SOR", "024", "Normal", "Near Mint")
        },
        Copy {
            deck: Some(luke_deck),
            ..copy("SOR", "070", "Normal", "Near Mint")
        },
        Copy {
            deck: Some(luke_deck),
            ..copy("SOR", "180", "Normal", "Near Mint")
        },
        // Loose owned copies — unassigned.
        Copy {
            price: Some(0.10),
            notes: Some("Wampa — undecided home."),
            ..copy("SOR", "085", "Normal", "Near Mint")
        },
        copy("SOR", "100", "Normal", "Heavily Played"),
        // The off-catalog copy.
        Copy {
            printing_id: misprint.printing_id.clone(),
            notes: Some("The misprint."),
            ..copy("SOR", "193", "Normal", "Near Mint")
        },
        // A sold copy — exercises the disposed-status path.
        Copy {
            status: "sold",
            price: Some(0.20),
            notes: Some("Sold to a friend."),
            ..copy("SOR", "130", "Foil", "Near Mint")
        },
    ];
    for c in &copies {
        let id = collection::add(
            conn,
            &NewCopy {
                printing_id: c.printing_id.clone(),
                condition: c.condition.into_some(),
                language: None,
                purchase_price: c.price,
                acquired_at: Some(ACQUIRED_AT.into()),
                source: c.source.into(),
                status: c.status.into_some(),
                notes: c.notes.map(str::to_owned),
                order_id: None,
                binder_id: c.binder,
                deck_id: c.deck,
                batch_id: c.batch,
            },
        )?;
        // `NewCopy` has no grading fields — apply the one graded card here.
        if c.printing_id == printing_id("SOR", "520", "Normal") {
            conn.execute(
                "UPDATE collection SET graded = 1, grade_company = 'PSA', \
                   grade_value = 9.0, grade_cert = '12345678' WHERE id = ?1",
                [id],
            )?;
        }
    }

    // --- Sealed collection --------------------------------------------
    sealed::add(
        conn,
        &NewSealed {
            product_id: 900002,
            quantity: Some(1),
            condition: "Near Mint".into_some(),
            purchase_price: Some(89.99),
            purchase_date: "2025-05-20".into_some(),
            source: "lgs".into_some(),
            seller_name: "Local Game Store".into_some(),
            notes: "Sealed Shadows of the Galaxy box — keeping factory sealed.".into_some(),
        },
    )?;
    sealed::add(
        conn,
        &NewSealed {
            product_id: 900004,
            quantity: Some(6),
            condition: "Near Mint".into_some(),
            purchase_price: Some(4.49),
            purchase_date: "2025-05-24".into_some(),
            source: "lgs".into_some(),
            seller_name: "Local Game Store".into_some(),
            notes: "Loose Jump to Lightspeed packs to rip.".into_some(),
        },
    )?;

    // --- Wishlist ------------------------------------------------------
    wishlist::add(
        conn,
        &NewWish {
            card_id: "SHD_001".into(),
            printing_id: printing_id("SHD", "515", "Normal").into_some(),
            max_price: Some(50.00),
            priority: Some(3),
            notes: "Showcase Cad Bane — top of the want list.".into_some(),
            ..Default::default()
        },
    )?;
    wishlist::add(
        conn,
        &NewWish {
            card_id: "JTL_001".into(),
            printing_id: None,
            max_price: Some(8.00),
            priority: Some(2),
            notes: "Any printing of Wedge.".into_some(),
            ..Default::default()
        },
    )?;
    wishlist::add(
        conn,
        &NewWish {
            card_id: "SOR_005".into(),
            printing_id: printing_id("SOR", "520", "Normal").into_some(),
            max_price: Some(90.00),
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

    /// One seeded fixture directory for the whole test binary. The fixture is
    /// not committed (it is built from these sources), so the tests that
    /// assert about its contents seed it themselves. `run` pins a process-wide
    /// clock, so every seed in this module goes through `SEED_LOCK`.
    static SEED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn seeded_dir() -> &'static Path {
        static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        DIR.get_or_init(|| {
            let dir = tempfile::tempdir().unwrap();
            let _g = SEED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            run(FixtureArgs {
                out: dir.path().to_path_buf(),
            })
            .expect("seed-fixture");
            dir
        })
        .path()
    }

    /// The seeded catalog fixture.
    fn committed_shared() -> PathBuf {
        seeded_dir().join("shared.sqlite")
    }

    /// The seeded collection fixture.
    fn committed_user() -> PathBuf {
        seeded_dir().join("collection.sqlite")
    }

    /// The instant `tests/visual/stabilize.ts` freezes the browser at. It
    /// lives in a TypeScript file and in this Rust one, and the two cannot
    /// share a constant, so it is written down twice on purpose — see
    /// `no_fixture_row_is_dated_after_the_frozen_browser_clock`.
    const FROZEN_BROWSER_CLOCK: &str = "2026-01-15";

    /// Seed a complete fixture into a fresh directory and return both files.
    fn seed_into(dir: &Path) -> (Vec<u8>, Vec<u8>) {
        let _g = SEED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
             is what a build timestamp looks like. Fix the seeder (fixture.rs).\n  {}",
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
    /// a fresh `open_shared` declares (the catalog schema and dumpstar-db's
    /// overlay together) — because it is the ONE database this
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
        assert!(!declared.is_empty(), "open_shared declared no objects");

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
            "tests/ui/fixtures/shared.sqlite is behind the schema open_shared \
             creates and is never repaired on open (it is ATTACHed read-only). \
             Missing: {}. Fix the seeder (fixture.rs).",
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
