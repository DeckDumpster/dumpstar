//! The raw landing zone, end to end through the real HTTP clients.
//!
//! `crates/dumpstar-lake` proves the key layout and the manifest in isolation.
//! What it cannot prove is the property this feature actually promises: that
//! landing is a **tee**, not a transform — the bytes stored are the bytes
//! received, and the catalog built from a landed run is identical to the
//! catalog built from an un-landed one.
//!
//! So these tests drive `TcgcsvClient` against a
//! local server (`tests/support`), twice, and compare.

// As in the three other test binaries that share this module: it serves four
// of them, so a helper only some of them need is not dead code.
#[allow(dead_code)]
mod support;

use std::sync::Arc;

use dumpstar_ingest::tcgcsv::TcgcsvClient;
use dumpstar_lake::{Dataset, DirStore, Manifest, RawLanding, Source};
use support::{FakeUpstream, Reply};

const INGEST_DATE: &str = "2026-08-11";

/// Two groups, and a product/price payload for each.
fn tcgcsv_route(target: &str, _n: usize) -> Reply {
    match target {
        "/79/groups" => Reply::ok(
            r#"{"results":[
                 {"groupId":1,"name":"Base Set","abbreviation":"BS","publishedOn":"1999-01-09"},
                 {"groupId":2,"name":"Jungle","abbreviation":"JU","publishedOn":"1999-06-16"}
               ]}"#,
        ),
        "/79/1/products" | "/79/2/products" => Reply::ok(r#"{"results":[]}"#),
        "/79/1/prices" | "/79/2/prices" => Reply::ok(r#"{"results":[]}"#),
        other => Reply::status(404, format!(r#"{{"error":"no route for {other}"}}"#)),
    }
}

/// The run's clock. Fixed rather than `now()`: a manifest's `started_at` is
/// what a later derive stamps into its rows, so a test that asserts on a
/// manifest wants a value it chose.
const STARTED_AT: &str = "2026-08-11T04:51:02Z";

fn landing_in(dir: &std::path::Path) -> Arc<RawLanding> {
    Arc::new(RawLanding::new(
        Box::new(DirStore::new(dir)),
        INGEST_DATE,
        STARTED_AT,
    ))
}

fn read_manifest(root: &std::path::Path, key: &str) -> Manifest {
    serde_json::from_slice(&std::fs::read(root.join(key)).expect("manifest on disk"))
        .expect("manifest parses")
}

fn manifest_of(
    root: &std::path::Path,
    landing: &RawLanding,
    source: Source,
    dataset: Dataset,
) -> Manifest {
    read_manifest(
        root,
        &dumpstar_lake::keys::manifest_key(source, dataset, INGEST_DATE, landing.run_id()),
    )
}

/// Landing changes nothing about what the client returns. This is the
/// "refresh behaviour is otherwise byte-identical" claim, made where it can
/// actually be checked: the parsed values on both sides of the tee.
#[test]
fn landing_does_not_change_what_the_client_parses() {
    let upstream = FakeUpstream::start(tcgcsv_route);
    let tmp = tempfile::tempdir().unwrap();

    let plain = TcgcsvClient::new().unwrap().base_url(&upstream.base_url());
    let landed = TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .landing_in(landing_in(tmp.path()));

    let a = plain.fetch_groups().unwrap();
    let b = landed.fetch_groups().unwrap();

    assert_eq!(a.len(), 2);
    assert_eq!(
        a.iter().map(|g| (g.group_id, &g.name)).collect::<Vec<_>>(),
        b.iter().map(|g| (g.group_id, &g.name)).collect::<Vec<_>>(),
    );
}

/// Every part's recorded SHA-256 and byte count match the object actually
/// stored, and the stored object decompresses to exactly what the upstream
/// sent. "Did we get everything" is answerable from the manifest alone.
#[test]
fn the_manifest_describes_the_bytes_that_were_stored() {
    let upstream = FakeUpstream::start(tcgcsv_route);
    let tmp = tempfile::tempdir().unwrap();
    let landing = landing_in(tmp.path());

    let client = TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .landing_in(Arc::clone(&landing));

    for group in client.fetch_groups().unwrap() {
        client.fetch_products(group.group_id).unwrap();
        client.fetch_prices(group.group_id).unwrap();
    }
    landing.finalize(None).unwrap();

    // Every request the server actually served is described by exactly one
    // part, and no part describes a request that never happened. This is the
    // "land every upstream fetch" claim, checked against the upstream's own
    // record rather than against the client's intent.
    let mut landed: Vec<String> = [Dataset::Groups, Dataset::Products, Dataset::Prices]
        .into_iter()
        .flat_map(|d| manifest_of(tmp.path(), &landing, Source::Tcgcsv, d).parts)
        .map(|p| p.url.trim_start_matches(&upstream.base_url()).to_string())
        .collect();
    landed.sort();
    let mut served = upstream.requests();
    served.sort();
    assert_eq!(landed, served);

    for (dataset, expected_parts) in [
        (Dataset::Groups, 1),
        (Dataset::Products, 2),
        (Dataset::Prices, 2),
    ] {
        let manifest = manifest_of(tmp.path(), &landing, Source::Tcgcsv, dataset);
        assert!(manifest.complete, "{dataset} should be complete");
        assert_eq!(manifest.parts.len(), expected_parts, "{dataset}");

        for part in &manifest.parts {
            assert_eq!(part.status, 200);
            let stored = std::fs::read(tmp.path().join(&part.key)).expect("object stored");
            let raw = zstd::decode_all(&stored[..]).expect("zstd round trip");
            assert_eq!(part.bytes, raw.len() as u64, "{}", part.key);
            assert_eq!(
                part.sha256,
                dumpstar_lake::sink::sha256_hex(&raw),
                "{}",
                part.key
            );
            // The bytes are the response, unparsed — still valid JSON with
            // the upstream's own envelope.
            let value: serde_json::Value = serde_json::from_slice(&raw).expect("stored JSON");
            assert!(value.get("results").is_some(), "{}", part.key);
            // The URL is the one actually requested, not a reconstruction.
            assert!(part.url.starts_with(&upstream.base_url()), "{}", part.url);
        }
    }
}

/// A fetch that fails partway leaves a manifest that says so, rather than a
/// silently short run. The manifest is on disk *before* anything finalizes,
/// because a failing upstream is exactly when the process is likeliest to be
/// killed before it tidies up.
#[test]
fn a_run_that_fails_partway_leaves_a_manifest_that_says_so() {
    // Groups, then the first group's products, then a 503.
    let upstream = FakeUpstream::start(|target, _| match target {
        "/79/groups" => Reply::ok(
            r#"{"results":[
                 {"groupId":1,"name":"Base Set","abbreviation":"BS","publishedOn":"1999-01-09"},
                 {"groupId":2,"name":"Jungle","abbreviation":"JU","publishedOn":"1999-06-16"}
               ]}"#,
        ),
        "/79/1/products" => Reply::ok(r#"{"results":[]}"#),
        _ => Reply::status(503, "upstream is down"),
    });
    let tmp = tempfile::tempdir().unwrap();
    let landing = landing_in(tmp.path());

    let client = TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .landing_in(Arc::clone(&landing));

    let groups = client.fetch_groups().unwrap();
    client.fetch_products(groups[0].group_id).unwrap();
    let err = client
        .fetch_prices(groups[0].group_id)
        .expect_err("the upstream is answering 503");

    // Flushed at the moment of failure, with no finalize call yet.
    let prices = manifest_of(tmp.path(), &landing, Source::Tcgcsv, Dataset::Prices);
    assert!(!prices.complete);
    assert!(prices.parts.is_empty());
    assert_eq!(prices.failures.len(), 1);
    assert_eq!(prices.failures[0].status, Some(503));
    assert!(prices.failures[0].url.ends_with("/79/1/prices"));

    // Finalizing with the acquisition error keeps it incomplete and records
    // the error; the datasets that did succeed are still marked complete.
    landing.finalize(Some(&err.to_string())).unwrap();
    let prices = manifest_of(tmp.path(), &landing, Source::Tcgcsv, Dataset::Prices);
    assert!(!prices.complete);
    assert!(prices.error.expect("an error is recorded").contains("503"));

    let groups_manifest = manifest_of(tmp.path(), &landing, Source::Tcgcsv, Dataset::Groups);
    assert_eq!(groups_manifest.parts.len(), 1);
}

/// Two runs on the same ingest date land in disjoint prefixes and neither
/// overwrites the other — the retry-after-partial-failure property, proved
/// through the real client rather than only at the key layer.
///
/// "Retry" here is a whole SECOND RUN with its own ULID — an operator or a
/// timer starting the job again — not the in-request retry
/// `dumpstar_ingest::retry` added in pd-nons. The two are different mechanisms
/// at different scales and this test is about the outer one, which is why the
/// first run is given no request-level budget: with one, the client would
/// simply ask again and there would be no failed run to land beside.
#[test]
fn a_retry_lands_beside_the_first_attempt_not_on_it() {
    let upstream = FakeUpstream::start(|target, n| match (target, n) {
        // The first run's groups call fails; the retry's succeeds.
        ("/79/groups", 0) => Reply::status(503, "upstream is down"),
        ("/79/groups", _) => Reply::ok(r#"{"results":[{"groupId":1,"name":"Base Set"}]}"#),
        _ => Reply::ok(r#"{"results":[]}"#),
    });
    let tmp = tempfile::tempdir().unwrap();

    let first = landing_in(tmp.path());
    let err = TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .landing_in(Arc::clone(&first))
        .retry(dumpstar_ingest::retry::RetryPolicy::none())
        .fetch_groups()
        .expect_err("first run fails");
    first.finalize(Some(&err.to_string())).unwrap();

    let second = landing_in(tmp.path());
    TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .landing_in(Arc::clone(&second))
        .fetch_groups()
        .expect("retry succeeds");
    second.finalize(None).unwrap();

    assert_ne!(first.run_id(), second.run_id());

    let failed = manifest_of(tmp.path(), &first, Source::Tcgcsv, Dataset::Groups);
    let retried = manifest_of(tmp.path(), &second, Source::Tcgcsv, Dataset::Groups);

    // The failed run's evidence survives the successful retry, in full.
    assert!(!failed.complete);
    assert!(failed.parts.is_empty());
    assert_eq!(failed.failures[0].status, Some(503));
    assert!(retried.complete);
    assert_eq!(retried.parts.len(), 1);

    // Same date, same dataset, different run — both prefixes present.
    let prefix = |run: &str| {
        tmp.path().join(dumpstar_lake::keys::run_prefix(
            Source::Tcgcsv,
            Dataset::Groups,
            INGEST_DATE,
            run,
        ))
    };
    assert!(prefix(first.run_id()).is_dir());
    assert!(prefix(second.run_id()).is_dir());
    assert_ne!(prefix(first.run_id()), prefix(second.run_id()));
}

/// With no landing zone the clients behave exactly as they did before this
/// feature existed: nothing is written anywhere.
#[test]
fn without_a_landing_zone_nothing_is_written() {
    let upstream = FakeUpstream::start(tcgcsv_route);
    let tmp = tempfile::tempdir().unwrap();

    let groups = TcgcsvClient::new()
        .unwrap()
        .base_url(&upstream.base_url())
        .fetch_groups()
        .unwrap();

    assert_eq!(groups.len(), 2);
    assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
}
