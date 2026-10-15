-- The Star Wars: Unlimited catalog schema (shared.sqlite), applied
-- idempotently by `Catalog::open_writer`. Written only by the catalog apply
-- job (a timer, never a request handler); request paths open the file with
-- `Catalog::open_readonly`.
--
-- Source-neutral: nothing here names an upstream. How an upstream's ids map
-- onto a printing lives in `printing_source_id`.

CREATE TABLE IF NOT EXISTS sets (
    set_code         TEXT PRIMARY KEY,          -- 'SOR'
    name             TEXT NOT NULL,
    release_date     TEXT,
    sort_order       INTEGER NOT NULL DEFAULT 0,
    category         TEXT                       -- SWU product category; see set_categories
);

-- A game card: everything the rules text says, shared by all its printings.
CREATE TABLE IF NOT EXISTS cards (
    card_id          TEXT PRIMARY KEY,          -- the official cardId
    name             TEXT NOT NULL,
    subtitle         TEXT,
    card_type        TEXT,                      -- 'Leader'|'Unit'|'Event'|...
    cost             INTEGER,
    power            INTEGER,
    hp               INTEGER,
    aspects          TEXT NOT NULL DEFAULT '[]', -- JSON array
    traits           TEXT NOT NULL DEFAULT '[]', -- JSON array
    arenas           TEXT NOT NULL DEFAULT '[]', -- JSON array
    rules_text       TEXT
);
CREATE INDEX IF NOT EXISTS idx_cards_name ON cards(name);

-- The official styled card text (textStyled, deployBoxStyled and
-- epicActionStyled, one <p> each, in that order), verbatim: HTML with the
-- publisher's inline game icons as <img> tags. A table of its own, not a
-- cards column, so a catalog written before it exists gains it by the next
-- apply instead of being rebuilt. The frontend sanitises it to the
-- icon/strong/em subset before render (sp-el3uaa).
CREATE TABLE IF NOT EXISTS card_text_styled (
    card_id          TEXT PRIMARY KEY REFERENCES cards(card_id),
    html             TEXT NOT NULL
);

-- One row per official printing, finish included: the Standard Foil of a
-- card is its own printing, as is its Hyperspace and Hyperspace Foil.
-- A printing is what the official API calls one (a cardUid). Two official
-- printings can share set, number, display variant and finish and differ only
-- in the serial code's variant digits (SEC lists `06361125` and `06371125`,
-- both 'Standard'), so `variant_code` is part of the natural key (sp-5zcm06).
CREATE TABLE IF NOT EXISTS printings (
    printing_id      TEXT PRIMARY KEY,
    card_id          TEXT NOT NULL REFERENCES cards(card_id),
    set_code         TEXT NOT NULL REFERENCES sets(set_code),
    number           TEXT NOT NULL,             -- collector number as printed
    variant          TEXT NOT NULL,             -- 'Standard'|'Hyperspace'|'Showcase'|'Promo'|...
    variant_code     TEXT NOT NULL DEFAULT '',  -- serialCode[2..4], '36'; '' when the source has none
    finish           TEXT NOT NULL CHECK (finish IN ('Normal', 'Foil')),
    rarity           TEXT,
    artist           TEXT,
    image_url        TEXT,
    UNIQUE (set_code, number, variant, variant_code, finish)
);
CREATE INDEX IF NOT EXISTS idx_printings_card ON printings(card_id);
CREATE INDEX IF NOT EXISTS idx_printings_set  ON printings(set_code);

-- The crosswalk: how each upstream names a printing.
--   source 'official': source_id = cardUid, sub_type_name = finish (a cardUid
--                      is shared by a printing's Normal and Foil)
--   source 'tcgcsv'  : source_id = productId, sub_type_name = subTypeName
CREATE TABLE IF NOT EXISTS printing_source_id (
    source           TEXT NOT NULL,
    source_id        TEXT NOT NULL,
    sub_type_name    TEXT NOT NULL DEFAULT '',
    printing_id      TEXT NOT NULL REFERENCES printings(printing_id),
    PRIMARY KEY (source, source_id, sub_type_name)
);
CREATE INDEX IF NOT EXISTS idx_psi_printing ON printing_source_id(printing_id);

-- Daily prices, append-only: one row per printing, source and day. The first
-- observation of a day stands; nothing is updated or deleted.
CREATE TABLE IF NOT EXISTS prices (
    printing_id      TEXT NOT NULL REFERENCES printings(printing_id),
    source           TEXT NOT NULL,
    observed_on      TEXT NOT NULL,             -- YYYY-MM-DD
    low              REAL,
    mid              REAL,
    high             REAL,
    market           REAL,
    direct_low       REAL,
    PRIMARY KEY (printing_id, source, observed_on)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_prices_day ON prices(observed_on);
CREATE TRIGGER IF NOT EXISTS prices_no_update BEFORE UPDATE ON prices
BEGIN SELECT RAISE(ABORT, 'prices is append-only'); END;
CREATE TRIGGER IF NOT EXISTS prices_no_delete BEFORE DELETE ON prices
BEGIN SELECT RAISE(ABORT, 'prices is append-only'); END;

-- Sealed products' own daily price series: tcgcsv products with no collector
-- number, keyed by TCGplayer productId. Never mixed into `prices`, which is the
-- card series. The first observation of a day stands.
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
CREATE TABLE IF NOT EXISTS formats (
    format_id        TEXT PRIMARY KEY,          -- 'premier'
    name             TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS format_rules (
    format_id        TEXT NOT NULL REFERENCES formats(format_id),
    rule_key         TEXT NOT NULL,             -- 'min_deck_size'
    rule_value       TEXT NOT NULL,
    PRIMARY KEY (format_id, rule_key)
);

CREATE TABLE IF NOT EXISTS card_legality (
    format_id        TEXT NOT NULL REFERENCES formats(format_id),
    card_id          TEXT NOT NULL REFERENCES cards(card_id),
    status           TEXT NOT NULL CHECK (status IN ('legal', 'banned', 'restricted', 'not_legal')),
    PRIMARY KEY (format_id, card_id)
) WITHOUT ROWID;

CREATE VIRTUAL TABLE IF NOT EXISTS card_search USING fts5(
    card_id UNINDEXED, name, subtitle, rules_text, traits
);

-- Provenance: which raw payloads have been applied. Hash and URL only; a
-- clock here would make a second apply differ from the first.
CREATE TABLE IF NOT EXISTS applied_payloads (
    payload_hash     TEXT PRIMARY KEY,
    source           TEXT NOT NULL,
    url              TEXT NOT NULL
);

-- Alarms raised by the join and the apply step. A row is an open alarm; the
-- join's kinds are recomputed on every join and rows that no longer hold are
-- deleted. severity 'alarm' fails `catalog-check`; 'info' is counted only.
CREATE TABLE IF NOT EXISTS alarms (
    kind             TEXT NOT NULL,
    key              TEXT NOT NULL,
    severity         TEXT NOT NULL CHECK (severity IN ('alarm', 'info')),
    detail           TEXT NOT NULL,
    PRIMARY KEY (kind, key)
) WITHOUT ROWID;

-- Where each upstream's incremental fetch resumes. ISO-8601 strings only; a
-- write never moves a key backwards.
CREATE TABLE IF NOT EXISTS sync_state (
    key              TEXT PRIMARY KEY,
    value            TEXT NOT NULL
) WITHOUT ROWID;

-- tcgcsv's view, stored verbatim-ish so the join can be re-run whenever either
-- side changes. Sealed products are rows with a NULL number.
CREATE TABLE IF NOT EXISTS tcgcsv_groups (
    group_id         INTEGER PRIMARY KEY,
    name             TEXT NOT NULL,
    abbreviation     TEXT
);
CREATE TABLE IF NOT EXISTS tcgcsv_group_dates (
    group_id         INTEGER PRIMARY KEY,
    published_on     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS tcgcsv_products (
    product_id       INTEGER PRIMARY KEY,
    group_id         INTEGER NOT NULL,
    name             TEXT NOT NULL,
    number           TEXT
);
CREATE INDEX IF NOT EXISTS idx_tcgcsv_products_group ON tcgcsv_products(group_id);
-- The price sub types (`Normal`, `Foil`) tcgcsv has quoted for a product, kept
-- so the join can link a product under the sub type its prices arrive as.
CREATE TABLE IF NOT EXISTS tcgcsv_sub_types (
    product_id       INTEGER NOT NULL,
    sub_type_name    TEXT NOT NULL,
    PRIMARY KEY (product_id, sub_type_name)
) WITHOUT ROWID;
