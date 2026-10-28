-- The Star Wars: Unlimited game schema for dumpster-kit-catalog's `Game`
-- (`Swu::SCHEMA`): schema.sql minus what the kit owns (printing_source_id,
-- prices, applied_payloads, sync_state, alarms, sealed_products) and the
-- tcgcsv mirror tables its crosswalk replaces. Runs against an empty file only.
-- Written only by the catalog apply
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
    rules_text       TEXT,
    keywords         TEXT NOT NULL DEFAULT '[]', -- JSON array of {"name": "Raid", "value": 2|null}
    mechanics        TEXT NOT NULL DEFAULT '[]', -- JSON array of {"name": "force", "role": "produces|consumes|requires", "side": "front|deployed"?, "optional": bool}
    is_unique        INTEGER NOT NULL DEFAULT 0, -- 1 for a unique card (the bullet before its name)
    epic_action      TEXT                       -- a leader's or base's epic action text, when it has one
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
    back_image_url   TEXT,                      -- a leader's deployed (unit) side; NULL for one-sided cards
    UNIQUE (set_code, number, variant, variant_code, finish)
);
CREATE INDEX IF NOT EXISTS idx_printings_card ON printings(card_id);
CREATE INDEX IF NOT EXISTS idx_printings_set  ON printings(set_code);

-- The official rarity mark: the one-letter character, its colour and the
-- publisher's sort position, per rarity name. Written by the catalog apply;
-- `printings.rarity` names a row by `name`.
CREATE TABLE IF NOT EXISTS rarity_marks (
    name             TEXT PRIMARY KEY,          -- 'Legendary'
    character        TEXT NOT NULL,             -- 'L'
    color            TEXT NOT NULL,             -- '#00c8f3'
    sort_value       INTEGER NOT NULL
);

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


-- Where a sealed product's picture came from and whether it is in the image
-- store yet. `retrieved_on`/`file` are NULL until the refresh has fetched it;
-- a changed `source_url` resets them so the new picture is fetched. The pages
-- serve `file` from the store and never the source URL.
CREATE TABLE IF NOT EXISTS sealed_images (
    product_id       INTEGER PRIMARY KEY,
    source_url       TEXT NOT NULL,
    retrieved_on     TEXT,
    file             TEXT
);
