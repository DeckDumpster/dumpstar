//! A card's aspects are stored in the order the Standard entry lists them
//! (the printed order); other printings list them by aspect id and must not
//! reorder them (sp-mmro2r).

use dumpstar_swu::Catalog;
use dumpstar_swu::apply::apply;
use dumpstar_swu::landing::Provenance;
use dumpstar_swu::official;
use serde_json::{Value, json};

fn aspect(id: i64, name: &str) -> Value {
    json!({"id": id, "attributes": {"name": name}})
}

fn page(uid: &str, serial: &str, set: &str, variant: &str, aspects: &[(i64, &str)]) -> Vec<u8> {
    let list: Vec<Value> = aspects.iter().map(|(i, n)| aspect(*i, n)).collect();
    serde_json::to_vec(&json!({
        "data": [{"id": 1, "attributes": {
            "title": "Jango Fett", "subtitle": "Wily Mercenary",
            "cardNumber": 9, "cardUid": uid, "serialCode": serial,
            "updatedAt": "2026-02-05T23:41:28.094Z",
            "expansion": {"data": {"attributes": {"code": set, "name": set, "sortValue": 1}}},
            "variantTypes": {"data": [{"attributes": {"name": variant, "foil": false}}]},
            "rarity": {"data": {"attributes": {"name": "Uncommon"}}},
            "type": {"data": {"attributes": {"name": "Unit"}}},
            "aspects": {"data": list},
        }}],
        "meta": {"pagination": {"pageCount": 1}}
    }))
    .unwrap()
}

fn prov(n: u32) -> Provenance {
    Provenance {
        source: "official".into(),
        url: format!("https://official.test/page{n}"),
        sha256: format!("{n:064}"),
        fetched_at: "2026-10-16T00:00:00Z".into(),
        bytes: 0,
        as_of: None,
    }
}

fn stored(c: &Catalog) -> Vec<String> {
    let s: String = c
        .conn()
        .query_row("SELECT aspects FROM cards", [], |r| r.get(0))
        .unwrap();
    serde_json::from_str(&s).unwrap()
}

const PRINTED: [&str; 3] = ["Cunning", "Vigilance", "Villainy"];

#[test]
fn a_non_standard_entry_never_reorders_the_standard_entrys_aspects() {
    let standard = page(
        "1",
        "07010087",
        "LAW",
        "Standard",
        &[(17, "Cunning"), (2, "Vigilance"), (27, "Villainy")],
    );
    let promo = page(
        "2",
        "07w600009",
        "LAWP",
        "Weekly Play",
        &[(2, "Vigilance"), (17, "Cunning"), (27, "Villainy")],
    );
    assert!(official::parse_page(&standard).unwrap().payload.cards[0].from_standard);
    assert!(!official::parse_page(&promo).unwrap().payload.cards[0].from_standard);

    for order in [[0, 1], [1, 0]] {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
        let pages = [&standard, &promo];
        for (n, i) in order.iter().enumerate() {
            apply(&mut c, pages[*i], &prov(n as u32 + 1)).unwrap();
        }
        assert_eq!(stored(&c), PRINTED, "apply order {order:?}");
    }
}

#[test]
fn a_standard_entrys_source_order_is_stored_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Catalog::open_writer(&dir.path().join("shared.sqlite")).unwrap();
    let odd = page(
        "1",
        "07010087",
        "LAW",
        "Standard",
        &[(27, "Villainy"), (17, "Cunning"), (2, "Vigilance")],
    );
    apply(&mut c, &odd, &prov(1)).unwrap();
    assert_eq!(stored(&c), ["Villainy", "Cunning", "Vigilance"]);
}
