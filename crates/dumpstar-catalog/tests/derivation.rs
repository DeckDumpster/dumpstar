//! A change in what an apply derives from a source re-applies the payloads
//! already applied, from the raw already landed (sp-boex8e).

use chrono::{TimeZone, Utc};
use dumpstar_catalog::Catalog;
use dumpstar_catalog::apply::apply_at;
use dumpstar_catalog::landing::land;
use dumpstar_catalog::source::RawPayload;
use dumpstar_catalog::swu::SRC_PRODUCTS;

fn images(c: &Catalog) -> i64 {
    c.conn()
        .query_row("SELECT COUNT(*) FROM sealed_images", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn a_bumped_derivation_reapplies_an_applied_payload_once_and_only_then() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("raw");
    let db = dir.path().join("shared.sqlite");
    let bytes = br#"{"success":true,"results":[{"productId":9001,"groupId":1,"name":"Spark of Rebellion Booster Box","imageUrl":"https://img.test/9001.jpg","extendedData":[]}]}"#.to_vec();
    let payload = RawPayload {
        source: SRC_PRODUCTS.into(),
        url: "https://tcgcsv.test/tcgplayer/79/1/products".into(),
        bytes: bytes.clone(),
        as_of: Some("2026-10-15".into()),
    };
    let landed = land(
        &raw,
        &payload,
        Utc.with_ymd_and_hms(2026, 10, 15, 21, 0, 0).unwrap(),
    )
    .unwrap();
    let mut c = Catalog::open_writer(&db).unwrap();

    // The earlier build derived no image rows: model it by clearing them.
    apply_at(&mut c, &bytes, &landed.provenance, 1).unwrap();
    c.conn().execute("DELETE FROM sealed_images", []).unwrap();
    assert_eq!(images(&c), 0);
    assert_eq!(
        c.reapply_landed_at(&raw, SRC_PRODUCTS, 1).unwrap(),
        0,
        "unchanged source is not re-applied"
    );
    assert_eq!(images(&c), 0);

    assert_eq!(c.reapply_landed_at(&raw, SRC_PRODUCTS, 2).unwrap(), 1);
    assert_eq!(images(&c), 1);
    assert_eq!(
        c.reapply_landed_at(&raw, SRC_PRODUCTS, 2).unwrap(),
        0,
        "once per version"
    );
}
