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
    sort_order       INTEGER NOT NULL DEFAULT 0
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

-- One row per official printing, finish included: the Standard Foil of a
-- card is its own printing, as is its Hyperspace and Hyperspace Foil.
CREATE TABLE IF NOT EXISTS printings (
    printing_id      TEXT PRIMARY KEY,
    card_id          TEXT NOT NULL REFERENCES cards(card_id),
    set_code         TEXT NOT NULL REFERENCES sets(set_code),
    number           TEXT NOT NULL,             -- collector number as printed
    variant          TEXT NOT NULL,             -- 'Standard'|'Hyperspace'|'Showcase'|'Promo'|...
    finish           TEXT NOT NULL CHECK (finish IN ('Normal', 'Foil')),
    rarity           TEXT,
    artist           TEXT,
    image_url        TEXT,
    UNIQUE (set_code, number, variant, finish)
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
