-- What dumpstar-db layers on top of the SWU catalog schema
-- (crates/dumpstar-catalog/src/schema.sql — the one card model). Applied on
-- every read-write open of shared.sqlite, after the catalog schema.
--
-- Nothing here describes a card: sets, cards, printings, prices, formats and
-- the full-text index are the catalog's. This file holds the search query
-- language's registry, the views the application reads sealed products
-- through, and the two provenance tables. A shared.sqlite written by the
-- Pokémon-shaped schema this project was forked with is emptied by
-- `dumpstar_catalog::reset_if_legacy` before either is applied.

-- ---------------------------------------------------------------------
-- Search query language metadata (data-model-is-the-product; decision D1/D2).
-- Seeded from data/search_keywords.json, data/rarities.json,
-- data/search_flags.json by dumpstar_db::search_meta::reconcile.
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS search_keywords (
    canonical   TEXT PRIMARY KEY,   -- 'aspect'
    aliases     TEXT NOT NULL,      -- JSON array, e.g. ["a"]
    operators   TEXT NOT NULL,      -- JSON array, e.g. [":","=","!="]
    kind        TEXT NOT NULL,      -- value class the compiler dispatches on
    target      TEXT,               -- column or JSON path, e.g. 'cards.aspects'
    value_enum  TEXT,               -- optional value-set reference
    semantics   TEXT,               -- compiler semantics tag ('exists','rank',…)
    help        TEXT                -- one-line help / autocomplete description
);

CREATE TABLE IF NOT EXISTS rarities (
    name   TEXT PRIMARY KEY,        -- 'Legendary'
    rank   INTEGER NOT NULL,        -- curated ordinal for r>= / r<
    grp    TEXT                     -- group alias
);

CREATE TABLE IF NOT EXISTS search_flags (
    flag       TEXT PRIMARY KEY,    -- 'foil'
    kind       TEXT NOT NULL,       -- 'variant_match' | 'finish' | 'computed'
    match_str  TEXT,                -- substring for variant_match flags
    predicate  TEXT,                -- predicate id for computed flags
    help       TEXT
);

-- ---------------------------------------------------------------------
-- Sealed products (tcgcsv products that carry no collector number).
-- Backfilled from tcgcsv_products on every convergence
-- (connection.rs::backfill_sealed); the catalog apply job does not write them.
-- ---------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS sealed_products (
    product_id      INTEGER PRIMARY KEY,       -- TCGplayer productId
    set_code        TEXT REFERENCES sets(set_code),
    name            TEXT NOT NULL,
    category        TEXT NOT NULL,             -- 'sealed' until a finer split exists
    subtype         TEXT,
    release_date    TEXT,
    image_url       TEXT,
    tcgplayer_url   TEXT,
    fetched_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sealed_products_set      ON sealed_products(set_code);
CREATE INDEX IF NOT EXISTS idx_sealed_products_category ON sealed_products(category);

CREATE TABLE IF NOT EXISTS sealed_prices (
    id                    INTEGER PRIMARY KEY AUTOINCREMENT,
    tcgplayer_product_id  INTEGER NOT NULL,
    low_price             REAL,
    mid_price             REAL,
    high_price            REAL,
    market_price          REAL,
    direct_low_price      REAL,
    observed_at           TEXT NOT NULL,
    UNIQUE(tcgplayer_product_id, observed_at)
);
CREATE INDEX IF NOT EXISTS idx_sealed_prices_product ON sealed_prices(tcgplayer_product_id);
CREATE INDEX IF NOT EXISTS idx_sealed_prices_date    ON sealed_prices(observed_at);

CREATE VIEW IF NOT EXISTS latest_sealed_prices AS
SELECT sp.* FROM sealed_prices sp
JOIN (SELECT tcgplayer_product_id, MAX(observed_at) AS observed_at
      FROM sealed_prices GROUP BY 1) m
  ON sp.tcgplayer_product_id = m.tcgplayer_product_id
 AND sp.observed_at = m.observed_at;

-- ---------------------------------------------------------------------
-- Provenance
-- ---------------------------------------------------------------------

-- Written ONLY by the offline derive (`dumpstar-lake-derive shared`), one row
-- per (ingest_date, source, dataset) it replayed. Delete-then-insert per
-- ingest_date, so re-deriving a date replaces that date's provenance.
CREATE TABLE IF NOT EXISTS raw_derivation (
    ingest_date  TEXT    NOT NULL,           -- the raw/ partition derived from
    source       TEXT    NOT NULL,
    dataset      TEXT    NOT NULL,
    run_id       TEXT    NOT NULL,           -- the run= ULID that was replayed
    parts        INTEGER NOT NULL,
    complete     INTEGER NOT NULL,           -- the manifest's complete flag
    observed_at  TEXT    NOT NULL,           -- the run's clock day
    derived_at   TEXT    NOT NULL,           -- when this derive ran, RFC 3339
    PRIMARY KEY (ingest_date, source, dataset)
);

-- One row: a SHA-256 over every embedded input the build that last opened this
-- catalog read-write would write into it (see convergence.rs). Written LAST by
-- open_shared. The hash and nothing else: a clock here would make two derives
-- of one partition differ in a field nobody reads.
CREATE TABLE IF NOT EXISTS catalog_convergence (
    id           INTEGER PRIMARY KEY CHECK (id = 1),
    fingerprint  TEXT NOT NULL
);
