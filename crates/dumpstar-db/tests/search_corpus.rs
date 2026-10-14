//! Tier 1 — curated corpus. Every entry asserts the parser/compiler verdict
//! and, for supported queries, that the SQL executes against a fixture DB.
//!
//! Contract per entry (`fixtures/search_corpus.json`):
//! - `should_parse=false`  → `parse` must error.
//! - `should_parse=true`   → `parse` must succeed, `compile` + `search` execute.
//! - `compiler_supported`  → the compiled WHERE has no `1=0` sentinel; otherwise
//!   it must contain one (the documented parse-but-unsupported cases).

use dumpstar_core::query::{KeywordRegistry, parse};
use dumpstar_db::search::{compile, search};
use dumpstar_db::search_meta::{self, SearchFlag};
use dumpstar_db::{connect_user, open_shared};
use rusqlite::Connection;
use serde::Deserialize;

const CORPUS: &str = include_str!("fixtures/search_corpus.json");

#[derive(Deserialize)]
struct Entry {
    query: String,
    #[allow(dead_code)]
    category: String,
    should_parse: bool,
    compiler_supported: bool,
}

/// A small but column-complete fixture: three cards across two sets, a deck,
/// a binder, and three collection copies (one graded + bindered, one loose,
/// one decked) so every keyword's SQL executes over real data.
fn fixture() -> (
    tempfile::TempDir,
    Connection,
    KeywordRegistry,
    Vec<SearchFlag>,
) {
    let dir = tempfile::tempdir().unwrap();
    let shared = dir.path().join("shared.sqlite");
    {
        let mut c = open_shared(&shared).unwrap();
        search_meta::reconcile(&mut c).unwrap();
        c.execute(
            "INSERT INTO sets (set_code, name, release_date, sort_order)
             VALUES ('SOR','Spark of Rebellion','2024-03-08',1),
                    ('SHD','Shadows of the Galaxy','2024-07-12',2)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO cards (card_id,name,subtitle,card_type,cost,power,hp,aspects,traits,
                arenas,rules_text)
             VALUES
             ('luke','Luke Skywalker','Faithful Friend','Unit',3,3,5,
               '[\"Vigilance\",\"Heroism\"]','[\"Force\",\"Rebel\",\"Jedi\"]','[\"Ground\"]',
               'On Attack: Heal 2 damage from a base.'),
             ('falcon','Millennium Falcon','Piece of Junk','Unit',2,3,3,
               '[\"Cunning\",\"Heroism\"]','[\"Vehicle\",\"Transport\"]','[\"Space\"]',
               'Piloting [2].'),
             ('vanquish','Vanquish',NULL,'Event',5,NULL,NULL,
               '[\"Vigilance\"]','[]','[]','Defeat a non-leader unit.')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO printings (printing_id,card_id,set_code,number,variant,finish,rarity,artist)
             VALUES
             ('luke-foil','luke','SOR','005','Standard','Foil','Legendary','Borja Pindado'),
             ('falcon-std','falcon','SHD','252','Standard','Normal','Common','Ralph McQuarrie'),
             ('falcon-hyper','falcon','SHD','512','Hyperspace','Normal','Common','Ralph McQuarrie'),
             ('vanquish-std','vanquish','SOR','193','Standard','Normal','Uncommon',NULL)",
            [],
        )
        .unwrap();
    }
    let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
    conn.execute(
        "INSERT INTO decks (id,name,created_at,updated_at) VALUES (1,'Rebels','2026-01-01','2026-01-01')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO binders (id,name,created_at,updated_at) VALUES (1,'Trade','2026-01-01','2026-01-01')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO collection
           (printing_id,condition,language,purchase_price,sale_price,acquired_at,source,notes,
            tags,graded,grade_company,grade_value,status,binder_id,deck_id)
         VALUES
         ('luke-foil','Near Mint','English',200.0,NULL,'2024-01-01','manual_id','clean copy',
           '[\"favorite\"]',1,'PSA',10.0,'owned',1,NULL),
         ('luke-foil','Lightly Played','English',150.0,NULL,'2024-02-01','manual_id',NULL,
           NULL,0,NULL,NULL,'owned',NULL,NULL),
         ('falcon-std','Near Mint','English',1.0,5.0,'2025-03-01','manual_id',NULL,
           NULL,0,NULL,NULL,'owned',NULL,1)",
        [],
    )
    .unwrap();
    let registry = search_meta::load_registry(&conn).unwrap();
    let flags = search_meta::load_flags(&conn).unwrap();
    (dir, conn, registry, flags)
}

#[test]
fn corpus_parses_compiles_and_executes() {
    let (_dir, conn, registry, flags) = fixture();
    let entries: Vec<Entry> = serde_json::from_str(CORPUS).expect("corpus JSON parses");
    assert!(
        entries.len() >= 150,
        "corpus should be comprehensive, got {}",
        entries.len()
    );

    for e in &entries {
        let parsed = parse(&e.query, &registry);

        if !e.should_parse {
            assert!(
                parsed.is_err(),
                "expected {:?} to FAIL parsing, but it parsed",
                e.query
            );
            continue;
        }

        let ast = parsed.unwrap_or_else(|err| panic!("expected {:?} to parse: {err}", e.query));
        let compiled = compile(&ast, &flags);

        if e.compiler_supported {
            assert!(
                compiled.is_supported(),
                "expected {:?} to compile without a 1=0 sentinel; where_sql = {}",
                e.query,
                compiled.where_sql()
            );
        } else {
            assert!(
                !compiled.is_supported(),
                "expected {:?} to compile to a 1=0 sentinel (unsupported value/flag)",
                e.query
            );
        }

        // Every parseable query must produce valid, executable SQL.
        search(&conn, &compiled)
            .unwrap_or_else(|err| panic!("execution failed for {:?}: {err:?}", e.query));
    }
}
