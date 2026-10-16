//! Binder-page assembly: a set rendered as ordered slots, one per collector
//! number, with per-printing ownership and master-set progress (PLAN.md §6).

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension};

use crate::error::Result;
use crate::search_meta::RarityLookup;

/// Header shown above a binder.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BinderSetInfo {
    pub set_code: String,
    pub name: String,
    pub release_date: Option<String>,
    /// The set's official logo and art crop, served from `/art`; filled by the
    /// server, `null` when none was retrieved.
    pub logo_url: Option<String>,
    pub art_url: Option<String>,
}

/// One printing within a slot.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SlotPrinting {
    pub printing_id: String,
    pub variant: String,
    /// `Normal` or `Foil`.
    pub finish: String,
    #[ts(type = "number")]
    pub owned_count: i64,
    pub market_price: Option<f64>,
}

/// One binder slot — a single collector number and all its printings (the
/// Normal and the Foil of it).
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BinderSlot {
    pub card_id: String,
    pub number: String,
    pub name: String,
    pub subtitle: Option<String>,
    pub rarity: Option<String>,
    pub image_url: Option<String>,
    /// `base` (Standard) | `hyperspace` | `showcase` | `promo` (anything
    /// else: Promo, Weekly Play, …).
    pub section: String,
    /// The catalog's card type (`Leader`, `Base`, `Unit`, …).
    pub card_type: Option<String>,
    /// Leaders and bases are printed landscape; the page lays them out apart
    /// from the portrait cards.
    pub landscape: bool,
    pub printings: Vec<SlotPrinting>,
}

/// A single rendered binder page with master-set progress.
/// Most cards a binder page holds: 8 per row (the set page's column maximum)
/// times its 3 rows. The frontend's `MAX_COLS * ROWS` is pinned to this by
/// `frontend/tests/browse-view.test.js`.
pub const BINDER_MAX_LAYOUT: i64 = 24;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BinderPage {
    pub set: BinderSetInfo,
    #[ts(type = "number")]
    pub layout: i64,
    #[ts(type = "number")]
    pub page: i64,
    #[ts(type = "number")]
    pub total_pages: i64,
    /// Standard-variant slot count, and how many of those the user owns.
    #[ts(type = "number")]
    pub base_total: i64,
    #[ts(type = "number")]
    pub base_owned: i64,
    /// Printing count across every visible section, and how many are owned.
    #[ts(type = "number")]
    pub master_total: i64,
    #[ts(type = "number")]
    pub master_owned: i64,
    pub slots: Vec<BinderSlot>,
}

/// How to sort, filter, and paginate a binder page.
#[derive(Debug, Clone)]
pub struct BinderQuery {
    pub page: i64,
    pub layout: i64,
    pub include_hyperspace: bool,
    pub include_showcase: bool,
    pub include_promos: bool,
    /// `number` (asc) | `number_desc` | `name` (A→Z) | `name_desc` |
    /// `price` (high→low) | `price_asc` | `rarity` (rare→common) |
    /// `rarity_asc`. Unknown values fall back to `number`.
    pub sort: String,
    /// Case-insensitive card-name substring. Empty means no search.
    pub search: String,
    /// `all` | `have` (own ≥1 printing) | `need` (own none) |
    /// `dupes` (own ≥2 of some printing).
    pub filter: String,
}

impl Default for BinderQuery {
    fn default() -> Self {
        Self {
            page: 1,
            layout: 9,
            include_hyperspace: true,
            include_showcase: true,
            include_promos: true,
            sort: "number".to_string(),
            search: String::new(),
            filter: "all".to_string(),
        }
    }
}

/// Position of a landscape card type in the top section: Leaders, then Bases.
fn landscape_rank(card_type: Option<&str>) -> Option<u8> {
    match card_type {
        Some("Leader") => Some(0),
        Some("Base") => Some(1),
        _ => None,
    }
}

fn section_of(variant: &str) -> &'static str {
    match variant {
        "Standard" => "base",
        "Hyperspace" => "hyperspace",
        "Showcase" => "showcase",
        _ => "promo",
    }
}

/// A slot's price for sorting: the price the tile shows (the frontend's `slotPrice`): the dearest *owned*
/// printing's, else the dearest printing's; `None` when nothing is priced.
fn slot_price(slot: &BinderSlot) -> Option<f64> {
    let best = |owned_only: bool| {
        slot.printings
            .iter()
            .filter(|p| !owned_only || p.owned_count > 0)
            .filter_map(|p| p.market_price)
            .reduce(f64::max)
    };
    best(true).or_else(|| best(false))
}

/// Order by price with unpriced slots last in either direction. Stable, so
/// ties keep collector-number order.
fn cmp_price(a: &BinderSlot, b: &BinderSlot, desc: bool) -> std::cmp::Ordering {
    match (slot_price(a), slot_price(b)) {
        (Some(x), Some(y)) if desc => y.total_cmp(&x),
        (Some(x), Some(y)) => x.total_cmp(&y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

/// Assemble a binder page for `set_code`. `None` if the set is unknown.
pub fn get_binder_page(
    conn: &Connection,
    set_code: &str,
    q: &BinderQuery,
) -> Result<Option<BinderPage>> {
    let set: Option<BinderSetInfo> = conn
        .prepare("SELECT set_code, name, release_date FROM sets WHERE set_code = ?1")?
        .query_row([set_code], |r| {
            Ok(BinderSetInfo {
                set_code: r.get(0)?,
                name: r.get(1)?,
                release_date: r.get(2)?,
                logo_url: None,
                art_url: None,
            })
        })
        .optional()?;
    let Some(set) = set else {
        return Ok(None);
    };
    let tiers = RarityLookup::load(conn)?;

    // Every printing in the set — catalog and the tenant's own — with its
    // card, owned count and market price. Grouped into slots by collector
    // number below; the catalog rows come first so a slot's identity is the
    // catalog's.
    let mut slots_by_number: HashMap<String, BinderSlot> = HashMap::new();
    {
        let mut stmt = conn.prepare(concat!(
            "SELECT p.number, p.card_id, cd.name, cd.subtitle, p.rarity, p.image_url, \
                    p.variant, p.printing_id, p.finish, \
                    (SELECT count(*) FROM collection c WHERE c.printing_id = p.printing_id), \
                    cd.card_type, ",
            crate::market_price_expr!(),
            " \
             FROM ",
            crate::printing_rel!(),
            " p \
             JOIN cards cd ON p.card_id = cd.card_id \
             WHERE p.set_code = ?1 \
             ORDER BY p.variant_description IS NOT NULL, p.variant, p.finish, p.printing_id",
        ))?;
        let rows = stmt.query_map([set_code], |r| {
            Ok((
                BinderSlot {
                    number: r.get(0)?,
                    card_id: r.get(1)?,
                    name: r.get(2)?,
                    subtitle: r.get(3)?,
                    rarity: r.get(4)?,
                    image_url: r.get(5)?,
                    section: section_of(&r.get::<_, String>(6)?).to_string(),
                    card_type: r.get(10)?,
                    landscape: false,
                    printings: Vec::new(),
                },
                SlotPrinting {
                    variant: r.get(6)?,
                    printing_id: r.get(7)?,
                    finish: r.get(8)?,
                    owned_count: r.get(9)?,
                    market_price: r.get(11)?,
                },
            ))
        })?;
        for row in rows {
            let (mut slot, printing) = row?;
            slot.landscape = landscape_rank(slot.card_type.as_deref()).is_some();
            slots_by_number
                .entry(slot.number.clone())
                .or_insert(slot)
                .printings
                .push(printing);
        }
    }
    let mut all: Vec<BinderSlot> = slots_by_number.into_values().collect();
    all.sort_by(|a, b| {
        dumpstar_core::number_sort_key(&a.number)
            .cmp(&dumpstar_core::number_sort_key(&b.number))
            .then_with(|| a.number.cmp(&b.number))
    });

    // Build every visible slot (all sections the include flags allow).
    let visible: Vec<BinderSlot> = all
        .into_iter()
        .filter(|slot| match slot.section.as_str() {
            "hyperspace" => q.include_hyperspace,
            "showcase" => q.include_showcase,
            "promo" => q.include_promos,
            _ => true, // base
        })
        .collect();

    // Master-set progress.
    let base_total = visible.iter().filter(|s| s.section == "base").count() as i64;
    let base_owned = visible
        .iter()
        .filter(|s| s.section == "base" && s.printings.iter().any(|p| p.owned_count > 0))
        .count() as i64;
    let master_total: i64 = visible.iter().map(|s| s.printings.len() as i64).sum();
    let master_owned = visible
        .iter()
        .flat_map(|s| &s.printings)
        .filter(|p| p.owned_count > 0)
        .count() as i64;

    // Sort. Progress above is computed pre-sort/filter, so the bars always
    // reflect the whole section-included set rather than the current view.
    // `visible` arrives in collector-number order.
    // Under any sort but collector number, leaders and bases form their own
    // top section: pulled out first, Leaders then Bases, each ordered by
    // the chosen sort (collector number breaks ties). Collector-number sorts
    // list every card strictly by number, leaders and bases included.
    let mut top: Vec<BinderSlot> = Vec::new();
    let mut rest: Vec<BinderSlot> = Vec::new();
    let by_number = matches!(q.sort.as_str(), "number" | "number_desc");
    for s in visible {
        if s.landscape && !by_number {
            top.push(s);
        } else {
            rest.push(s);
        }
    }
    let rank = |s: &BinderSlot| s.rarity.as_deref().map_or(-1, |r| tiers.sort_value(r));
    let apply_sort = |v: &mut Vec<BinderSlot>| match q.sort.as_str() {
        "number_desc" => v.reverse(),
        "name" => v.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
        }),
        "name_desc" => v.sort_by(|a, b| {
            b.name
                .to_ascii_lowercase()
                .cmp(&a.name.to_ascii_lowercase())
        }),
        "price" => v.sort_by(|a, b| cmp_price(a, b, true)),
        "price_asc" => v.sort_by(|a, b| cmp_price(a, b, false)),
        "rarity" => v.sort_by_key(|s| std::cmp::Reverse(rank(s))),
        "rarity_asc" => v.sort_by_key(|s| rank(s)),
        _ => {} // "number" — already collector-number ascending.
    };
    // The chosen sort orders inside the top section too (Leaders, then
    // Bases): a set with dozens of them would otherwise show the same fixed
    // pages under every sort.
    apply_sort(&mut top);
    top.sort_by_key(|s| landscape_rank(s.card_type.as_deref()));
    let mut visible = rest;
    apply_sort(&mut visible);
    visible.splice(0..0, top);

    // Filter: in-set name search, then the ownership tab.
    let search = q.search.trim().to_ascii_lowercase();
    let owns_one = |s: &BinderSlot| s.printings.iter().any(|p| p.owned_count > 0);
    visible.retain(|s| {
        if !search.is_empty() && !s.name.to_ascii_lowercase().contains(&search) {
            return false;
        }
        match q.filter.as_str() {
            "have" => owns_one(s),
            "need" => !owns_one(s),
            "dupes" => s.printings.iter().any(|p| p.owned_count >= 2),
            _ => true, // "all"
        }
    });

    // Paginate the sorted, filtered view.
    let layout = q.layout.clamp(1, BINDER_MAX_LAYOUT);
    let visible_count = visible.len() as i64;
    let total_pages = ((visible_count + layout - 1) / layout).max(1);
    let page = q.page.clamp(1, total_pages);
    let start = ((page - 1) * layout) as usize;
    let slots: Vec<BinderSlot> = visible
        .into_iter()
        .skip(start)
        .take(layout as usize)
        .collect();
    Ok(Some(BinderPage {
        set,
        layout,
        page,
        total_pages,
        base_total,
        base_owned,
        master_total,
        master_owned,
        slots,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collection::{self, NewCopy};
    use crate::testkit;
    use crate::{connect_user, open_shared};

    /// Spark of Rebellion, as a binder sees it: base #1-#3, Hyperspace #264
    /// (of card 1), Showcase #270 (of card 2) and a promo #P01. #1 has a
    /// Foil too, so its slot holds two printings.
    fn binder_conn() -> (tempfile::TempDir, Connection) {
        binder_conn_priced(&[])
    }

    fn binder_conn_priced(prices: &[(&str, f64)]) -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::set(&c, "SOR", "Spark of Rebellion");
            testkit::simple(&c, "SOR", "1", "Luke Skywalker");
            testkit::foil_of(&c, "SOR", "1");
            testkit::simple(&c, "SOR", "2", "Millennium Falcon");
            testkit::simple(&c, "SOR", "3", "Vanquish");
            testkit::printing(
                &c,
                "SOR-1-hyper",
                "SOR-1",
                "SOR",
                "264",
                "Hyperspace",
                "Normal",
            );
            testkit::printing(
                &c,
                "SOR-2-show",
                "SOR-2",
                "SOR",
                "270",
                "Showcase",
                "Normal",
            );
            testkit::card(&c, "SOR-P01", "Darth Vader");
            testkit::printing(
                &c,
                "SOR-P01-promo",
                "SOR-P01",
                "SOR",
                "P01",
                "Promo",
                "Normal",
            );
            for (pid, v) in prices {
                testkit::price(&c, pid, "2026-05-18", *v);
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        (dir, conn)
    }

    fn own(conn: &mut Connection, pids: &[&str]) {
        for pid in pids {
            collection::add(
                conn,
                &NewCopy {
                    printing_id: (*pid).into(),
                    source: "manual_id".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        }
    }

    // Regression for dumpstar-3s2: a card owned only
    // through a custom variant (user_printings — the "Missing Variant" escape
    // hatch) must still highlight as owned in the binder, count toward
    // base_owned, and surface the custom variant as its own slot printing. The
    // binder query was catalog-only (printings, no user_printings), so the slot
    // stayed unowned.
    #[test]
    fn slot_owns_card_via_custom_variant() {
        use crate::user_printings::{self, NewUserPrinting};
        let (_d, mut conn) = binder_conn();

        // A custom variant on card #2, and one owned copy of it. Card #2 has no
        // owned catalog printing.
        let up = user_printings::insert(
            &conn,
            &NewUserPrinting {
                card_id: "SOR-2".into(),
                description: Some("inverted holo misprint".into()),
            },
        )
        .unwrap();
        assert_eq!(up.variant, "missing_variant");
        own(&mut conn, &[up.printing_id.as_str()]);

        let p = get_binder_page(&conn, "SOR", &BinderQuery::default())
            .unwrap()
            .unwrap();
        let slot = p
            .slots
            .iter()
            .find(|s| s.number == "2")
            .expect("card #2 slot present");
        assert_eq!(
            slot.section, "base",
            "the slot keeps the catalog's identity"
        );
        let custom = slot
            .printings
            .iter()
            .find(|pr| pr.printing_id == up.printing_id)
            .expect("custom variant surfaces as a slot printing");
        assert_eq!(custom.variant, "missing_variant");
        assert_eq!(custom.owned_count, 1, "the custom-variant copy is counted");
        assert!(
            slot.printings.iter().any(|pr| pr.owned_count > 0),
            "slot reads as owned"
        );
        // base_owned counts the card even though only its custom variant is owned.
        assert_eq!(p.base_owned, 1);
    }

    #[test]
    fn assembles_sections_and_progress() {
        let (_d, mut conn) = binder_conn();
        own(&mut conn, &["SOR-1-normal"]);

        // All sections on: one slot per collector number.
        let p = get_binder_page(&conn, "SOR", &BinderQuery::default())
            .unwrap()
            .unwrap();
        assert_eq!(p.set.name, "Spark of Rebellion");
        let numbers: Vec<&str> = p.slots.iter().map(|s| s.number.as_str()).collect();
        assert_eq!(numbers, ["1", "2", "3", "264", "270", "P01"]);
        let sections: Vec<&str> = p.slots.iter().map(|s| s.section.as_str()).collect();
        assert_eq!(
            sections,
            ["base", "base", "base", "hyperspace", "showcase", "promo"]
        );
        assert_eq!(p.base_total, 3);
        assert_eq!(p.base_owned, 1);
        // Standard #1 holds a Normal and a Foil; every other slot one printing.
        assert_eq!(p.slots[0].printings.len(), 2);
        assert_eq!(p.master_total, 7);
        assert_eq!(p.master_owned, 1);

        // Hyperspace + showcase + promos off -> only the 3 base slots.
        let base_only = get_binder_page(
            &conn,
            "SOR",
            &BinderQuery {
                include_hyperspace: false,
                include_showcase: false,
                include_promos: false,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(base_only.slots.len(), 3);
        assert!(base_only.slots.iter().all(|s| s.section == "base"));
        assert_eq!(base_only.master_total, 4);

        // Each include flag drops exactly its own section.
        let no_hyper = get_binder_page(
            &conn,
            "SOR",
            &BinderQuery {
                include_hyperspace: false,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(no_hyper.slots.len(), 5);
        assert!(no_hyper.slots.iter().all(|s| s.section != "hyperspace"));
        let no_promo = get_binder_page(
            &conn,
            "SOR",
            &BinderQuery {
                include_promos: false,
                ..Default::default()
            },
        )
        .unwrap()
        .unwrap();
        assert!(no_promo.slots.iter().all(|s| s.section != "promo"));
        assert_eq!(no_promo.slots.len(), 5);
    }

    #[test]
    fn slot_carries_card_identity_and_prices() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            testkit::card_full(
                &c,
                "SOR-5",
                "Luke Skywalker",
                Some("Jedi Knight"),
                "Unit",
                Some(4),
                Some(4),
                Some(5),
                "[]",
                "[]",
                "[]",
                None,
            );
            testkit::set(&c, "SOR", "Spark of Rebellion");
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
            c.execute(
                "UPDATE printings SET rarity = 'Legendary', image_url = 'https://img/l.png' \
                  WHERE card_id = 'SOR-5'",
                [],
            )
            .unwrap();
            testkit::price(&c, "SOR-5-normal", "2026-05-18", 2.0);
            testkit::price(&c, "SOR-5-foil", "2026-05-18", 9.0);
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let p = get_binder_page(&conn, "SOR", &BinderQuery::default())
            .unwrap()
            .unwrap();
        let slot = &p.slots[0];
        assert_eq!(slot.name, "Luke Skywalker");
        assert_eq!(slot.subtitle.as_deref(), Some("Jedi Knight"));
        assert_eq!(slot.rarity.as_deref(), Some("Legendary"));
        assert_eq!(slot.image_url.as_deref(), Some("https://img/l.png"));
        let price = |finish: &str| {
            slot.printings
                .iter()
                .find(|pr| pr.finish == finish)
                .unwrap()
                .market_price
        };
        assert_eq!(price("Normal"), Some(2.0));
        assert_eq!(price("Foil"), Some(9.0));
    }

    #[test]
    fn paginates() {
        let (_d, conn) = binder_conn();
        let q = |page: i64| BinderQuery {
            page,
            layout: 4,
            ..Default::default()
        };
        let p1 = get_binder_page(&conn, "SOR", &q(1)).unwrap().unwrap();
        assert_eq!(p1.total_pages, 2);
        assert_eq!(p1.slots.len(), 4);
        assert_eq!(p1.slots[0].number, "1");

        let p2 = get_binder_page(&conn, "SOR", &q(2)).unwrap().unwrap();
        assert_eq!(p2.slots.len(), 2);
        assert_eq!(p2.slots[0].number, "270");

        // Out-of-range page clamps.
        let clamped = get_binder_page(&conn, "SOR", &q(99)).unwrap().unwrap();
        assert_eq!(clamped.page, 2);
    }

    #[test]
    fn leaders_and_bases_head_the_page_in_every_non_number_sort() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            // Collector order interleaves types: base 1, leader 2, unit 3, ...
            let rows = [
                ("1", "Zeb Base", "Base"),
                ("2", "Yoda Leader", "Leader"),
                ("3", "Unit Echo", "Unit"),
                ("4", "Unit Delta", "Unit"),
                ("5", "Ahsoka Leader", "Leader"),
                ("6", "Unit Charlie", "Unit"),
                ("7", "Aldhani Base", "Base"),
                ("8", "Unit Bravo", "Unit"),
                ("9", "Unit Alpha", "Unit"),
                ("10", "Unit Foxtrot", "Unit"),
            ];
            for (n, name, ty) in rows {
                testkit::simple(&c, "SOR", n, name);
                c.execute(
                    "UPDATE cards SET card_type = ?1 WHERE card_id = ?2",
                    rusqlite::params![ty, format!("SOR-{n}")],
                )
                .unwrap();
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        // Collector-number sorts do not hoist: #1..#10 in order.
        for (sort, want) in [
            (
                "number",
                ["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"],
            ),
            (
                "number_desc",
                ["10", "9", "8", "7", "6", "5", "4", "3", "2", "1"],
            ),
        ] {
            let page = get_binder_page(
                &conn,
                "SOR",
                &BinderQuery {
                    sort: sort.into(),
                    layout: 60,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
            let nums: Vec<&str> = page.slots.iter().map(|s| s.number.as_str()).collect();
            assert_eq!(nums, want, "sort {sort}");
        }
        for sort in ["name", "name_desc", "price", "rarity"] {
            let page = get_binder_page(
                &conn,
                "SOR",
                &BinderQuery {
                    sort: sort.into(),
                    layout: 60,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap();
            let nums: Vec<&str> = page.slots.iter().map(|s| s.number.as_str()).collect();
            // Leaders then Bases, each ordered by the sort; the unpriced and
            // equal-rarity ties keep collector-number order.
            let want: [&str; 4] = match sort {
                "name" => ["5", "2", "7", "1"],
                "name_desc" => ["2", "5", "1", "7"],
                _ => ["2", "5", "1", "7"],
            };
            assert_eq!(&nums[..4], want, "sort {sort}");
            assert!(page.slots[..4].iter().all(|s| s.landscape), "sort {sort}");
            assert!(page.slots[4..].iter().all(|s| !s.landscape), "sort {sort}");
            if sort == "name" {
                let names: Vec<&str> = page.slots[4..].iter().map(|s| s.name.as_str()).collect();
                assert_eq!(
                    names,
                    [
                        "Unit Alpha",
                        "Unit Bravo",
                        "Unit Charlie",
                        "Unit Delta",
                        "Unit Echo",
                        "Unit Foxtrot"
                    ]
                );
            }
        }
    }

    #[test]
    fn price_sort_reorders_a_set_headed_by_leaders_and_bases() {
        // HMW shape: dozens of leaders and bases head the page, so the first
        // pages must differ between the two price directions.
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let c = open_shared(&shared).unwrap();
            for (i, (ty, price)) in [
                ("Leader", 0.5),
                ("Leader", 3.0),
                ("Leader", 1.0),
                ("Base", 0.2),
                ("Base", 0.9),
                ("Unit", 7.0),
            ]
            .into_iter()
            .enumerate()
            {
                let n = (i + 1).to_string();
                testkit::simple(&c, "SOR", &n, &format!("Card {n}"));
                c.execute(
                    "UPDATE cards SET card_type = ?1 WHERE card_id = ?2",
                    rusqlite::params![ty, format!("SOR-{n}")],
                )
                .unwrap();
                testkit::price(&c, &format!("SOR-{n}-normal"), "2026-05-18", price);
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let order = |sort: &str| -> Vec<String> {
            get_binder_page(
                &conn,
                "SOR",
                &BinderQuery {
                    sort: sort.into(),
                    layout: 60,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap()
            .slots
            .into_iter()
            .map(|s| s.number)
            .collect()
        };
        assert_eq!(order("price"), ["2", "3", "1", "5", "4", "6"]);
        assert_eq!(order("price_asc"), ["1", "3", "2", "4", "5", "6"]);
    }

    #[test]
    fn price_sort_orders_by_shown_price_unpriced_last() {
        // Priced: #3 $5, #264 $20, #2 $1. Unpriced: #1, #270, #P01.
        let (_d, conn) = binder_conn_priced(&[
            ("SOR-3-normal", 5.0),
            ("SOR-1-hyper", 20.0),
            ("SOR-2-normal", 1.0),
        ]);
        let order = |sort: &str| -> Vec<String> {
            get_binder_page(
                &conn,
                "SOR",
                &BinderQuery {
                    sort: sort.into(),
                    layout: 60,
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap()
            .slots
            .into_iter()
            .filter(|s| !s.landscape)
            .map(|s| s.number)
            .collect()
        };
        let priced = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .filter(|n| ["264", "3", "2"].contains(&n.as_str()))
                .collect()
        };
        let desc = order("price");
        let asc = order("price_asc");
        assert_eq!(priced(desc.clone()), ["264", "3", "2"]);
        assert_eq!(priced(asc.clone()), ["2", "3", "264"]);
        for v in [&desc, &asc] {
            let last_priced = v
                .iter()
                .rposition(|n| ["264", "3", "2"].contains(&n.as_str()));
            let first_unpriced = v
                .iter()
                .position(|n| !["264", "3", "2"].contains(&n.as_str()));
            assert!(last_priced < first_unpriced, "unpriced last: {v:?}");
        }
    }

    #[test]
    fn sorts_searches_and_filters() {
        let (_d, mut conn) = binder_conn();
        // Own #1 twice (a duplicate) and #2 once.
        own(&mut conn, &["SOR-1-normal", "SOR-1-normal", "SOR-2-normal"]);

        let q = |sort: &str, filter: &str, search: &str| BinderQuery {
            sort: sort.into(),
            filter: filter.into(),
            search: search.into(),
            ..Default::default()
        };

        // number_desc reverses collector order: the lettered promo is last
        // ascending, so first descending.
        let desc = get_binder_page(&conn, "SOR", &q("number_desc", "all", ""))
            .unwrap()
            .unwrap();
        assert_eq!(desc.slots[0].number, "P01");
        assert_eq!(desc.slots[5].number, "1");

        // Name sorts both ways.
        let by_name = get_binder_page(&conn, "SOR", &q("name", "all", ""))
            .unwrap()
            .unwrap();
        assert_eq!(by_name.slots[0].name, "Darth Vader");
        let by_name_desc = get_binder_page(&conn, "SOR", &q("name_desc", "all", ""))
            .unwrap()
            .unwrap();
        assert_eq!(by_name_desc.slots[0].name, "Vanquish");

        // The other sorts parse and produce a page.
        for s in ["price", "price_asc", "rarity", "rarity_asc"] {
            let page = get_binder_page(&conn, "SOR", &q(s, "all", "")).unwrap();
            assert!(page.is_some(), "sort '{s}' should produce a page");
        }

        // "need" -> slots with no owned printing (#3, #264, #270, #P01).
        let need = get_binder_page(&conn, "SOR", &q("number", "need", ""))
            .unwrap()
            .unwrap();
        let nums: Vec<&str> = need.slots.iter().map(|s| s.number.as_str()).collect();
        assert_eq!(nums, ["3", "264", "270", "P01"]);

        // "have" -> #1 and #2; progress still reflects the whole set.
        let have = get_binder_page(&conn, "SOR", &q("number", "have", ""))
            .unwrap()
            .unwrap();
        assert_eq!(have.slots.len(), 2);
        assert_eq!(have.base_total, 3, "progress is pre-filter");

        // "dupes" -> only #1 (owned twice).
        let dupes = get_binder_page(&conn, "SOR", &q("number", "dupes", ""))
            .unwrap()
            .unwrap();
        assert_eq!(dupes.slots.len(), 1);
        assert_eq!(dupes.slots[0].number, "1");

        // Search narrows by card name; a card's Hyperspace slot matches too.
        let hit = get_binder_page(&conn, "SOR", &q("number", "all", "LUKE"))
            .unwrap()
            .unwrap();
        let nums: Vec<&str> = hit.slots.iter().map(|s| s.number.as_str()).collect();
        assert_eq!(nums, ["1", "264"]);
        let miss = get_binder_page(&conn, "SOR", &q("number", "all", "zzz"))
            .unwrap()
            .unwrap();
        assert!(miss.slots.is_empty());
    }

    #[test]
    fn unknown_set_is_none() {
        let (_d, conn) = binder_conn();
        assert!(
            get_binder_page(&conn, "nope", &BinderQuery::default())
                .unwrap()
                .is_none()
        );
    }
}
