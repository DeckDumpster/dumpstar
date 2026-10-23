//! `/mcp` — the v1 agent-tool contract for Star Wars: Unlimited, over the kit's
//! `dumpster-kit-mcp` (protocol) and `dumpster-kit-mcp-auth` (Pocket ID bearer tokens).
//!
//! Every tool reaches the database through the same `dumpstar-db` functions `/api` uses and
//! through [`Tenants::connection`], with the tenant the auth layer resolved. The server holds no
//! deck-building judgement: it enforces ownership, reservation and sanctioned formats only.
//! The `search_cards` description and `query_help` are generated from `data/search_keywords.json`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, header};
use dumpster_kit_mcp::{
    ContextResolver, Keywords, McpAuthError, ResolveFuture, ToolContext, ToolDef, ToolError,
    ToolProvider,
};
use dumpster_kit_tenancy::Tenants;
use dumpster_kit_tenancy::tenant::TenantId;
use rusqlite::{Connection, OptionalExtension};
use serde_json::{Value, json};

use crate::dbpool;
use dumpstar_core::deck::{DeckEntry, Zone};
use dumpstar_core::query::{KeywordRegistry, parse};
use dumpstar_db::DbError;
use dumpstar_db::cards::{self, Card};
use dumpstar_db::decks::{self, CardCount, NewDeck, SetLines};
use dumpstar_db::search::{self, SearchRow, Slice};
use dumpstar_db::search_meta::SearchFlag;

/// Rows per `search_cards` page when the agent names none.
const DEFAULT_PAGE: u32 = 25;
/// The most rows one `search_cards` page returns; keeps a page far under the result-size cap.
const MAX_PAGE: u32 = 100;
const EXAMPLES: [&str; 4] = [
    "a:vigilance t:unit arena:space cost<=3",
    "t:leader a:heroism",
    "trait:rebel power>=4 -deck:*",
    "o:\"when played\" r>=rare s:sor",
];

pub struct DumpstarTools {
    tenants: Arc<Tenants>,
    registry: Arc<KeywordRegistry>,
    flags: Arc<Vec<SearchFlag>>,
    keywords: Keywords,
    ids: Arc<MintedTenants>,
}

/// The kit hands a provider the tenant as a string (a `TenantId` can only be minted by the
/// registry). The resolver records the id it was given under that string and the provider looks
/// it up, so no tenant is ever named from a request value.
#[derive(Default)]
pub struct MintedTenants(Mutex<HashMap<String, TenantId>>);

pub struct Resolver {
    ids: Arc<MintedTenants>,
}

impl ContextResolver for Resolver {
    fn resolve<'a>(&'a self, headers: &'a HeaderMap) -> ResolveFuture<'a> {
        Box::pin(async move {
            let id = dumpster_kit_tenancy::tenant::current()
                .map_err(|_| McpAuthError::unauthorized("not signed in", None))?;
            // The rate-limit key: one per bearer token, never the token itself.
            let token = headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let grant = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                token.hash(&mut h);
                format!("{:016x}", h.finish())
            };
            let tenant = id.as_str().to_string();
            self.ids
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(tenant.clone(), id);
            Ok(ToolContext { tenant, grant })
        })
    }
}

/// The provider and resolver for one server.
pub fn tools(
    tenants: Arc<Tenants>,
    registry: Arc<KeywordRegistry>,
    flags: Arc<Vec<SearchFlag>>,
) -> Result<(Arc<DumpstarTools>, Arc<Resolver>), String> {
    let keywords = Keywords::parse(&dumpstar_db::search_meta::keywords_json())?;
    let ids = Arc::new(MintedTenants::default());
    Ok((
        Arc::new(DumpstarTools {
            tenants,
            registry,
            flags,
            keywords,
            ids: ids.clone(),
        }),
        Arc::new(Resolver { ids }),
    ))
}

/// How every tool describes a card id.
const CARD_ID_DOC: &str = "Card id: <SET>-<number> of the card's original (earliest standard) printing, e.g. SOR-5. Older ids (n:<slug>, bare numbers) are still accepted.";

fn db_err(e: DbError) -> ToolError {
    match e {
        DbError::NotFound(_) | DbError::Conflict(_) | DbError::Invalid(_) | DbError::Import(_) => {
            ToolError::Failed(e.to_string())
        }
        other => ToolError::Internal(other.to_string()),
    }
}

fn failed(m: impl Into<String>) -> ToolError {
    ToolError::Failed(m.into())
}

fn arg_str<'a>(args: &'a Value, k: &str) -> Option<&'a str> {
    args.get(k)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

fn arg_id(args: &Value, k: &str) -> Result<i64, ToolError> {
    args.get(k)
        .and_then(Value::as_i64)
        .ok_or_else(|| failed(format!("'{k}' must be a whole number")))
}

fn json_array(raw: &Option<String>) -> Vec<String> {
    raw.as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default()
}

/// Alignment in the aspect names: Heroism, Villainy, otherwise Neutral.
fn alignment(aspects: &[String]) -> &'static str {
    ["Heroism", "Villainy"]
        .into_iter()
        .find(|n| aspects.iter().any(|a| a == n))
        .unwrap_or("Neutral")
}

struct Facts<'a> {
    card_id: &'a str,
    display_id: String,
    name: &'a str,
    subtitle: &'a Option<String>,
    set: &'a str,
    number: &'a str,
    rarity: &'a Option<String>,
    card_type: &'a Option<String>,
    cost: Option<i64>,
    power: Option<i64>,
    hp: Option<i64>,
    aspects: &'a Option<String>,
    arenas: &'a Option<String>,
    traits: &'a Option<String>,
    rules_text: Option<&'a str>,
}

/// The catalog's structured card text: keywords, uniqueness and the epic action.
struct Structured {
    keywords: Value,
    unique: bool,
    epic_action: Option<String>,
    rules_text: Option<String>,
}

/// Owned and available copy counts, keyed by label, as the breakdowns return them.
#[derive(Default)]
struct Tally(std::collections::BTreeMap<String, (u64, u64)>);

/// A `Tally` that also counts distinct cards: leaders are singleton, so a pack planner needs
/// how many different leaders it owns, not how many copies.
#[derive(Default)]
struct CardTally(std::collections::BTreeMap<String, (u64, u64, std::collections::HashSet<String>)>);

impl CardTally {
    fn add(&mut self, key: &str, card_id: &str, available: bool) {
        let e = self.0.entry(key.to_string()).or_default();
        e.0 += 1;
        e.1 += u64::from(available);
        e.2.insert(card_id.to_string());
    }

    fn json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(k, (o, a, d))| {
                    (
                        k.clone(),
                        json!({"distinct": d.len(), "owned": o, "available": a}),
                    )
                })
                .collect(),
        )
    }
}

impl Tally {
    fn add(&mut self, key: &str, available: bool) {
        let e = self.0.entry(key.to_string()).or_default();
        e.0 += 1;
        e.1 += u64::from(available);
    }

    fn json(&self) -> Value {
        Value::Object(
            self.0
                .iter()
                .map(|(k, (o, a))| (k.clone(), json!({"owned": o, "available": a})))
                .collect(),
        )
    }
}

/// Owned and available copies by type, aspect and set; bases by HP and epic, ability or vanilla;
/// leaders by aspect, alignment and color x alignment (`none` for a leader with no color; Neutral
/// is its own alignment). A card with two aspects counts under each.
fn breakdown(conn: &Connection) -> Result<Value, ToolError> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT cd.card_id, cd.card_type, cd.aspects, cd.hp, cd.epic_action, cd.rules_text, \
                    (SELECT set_code FROM shared.printings WHERE card_id = cd.card_id \
                      {CANONICAL_PRINTING}), c.deck_id IS NULL \
               FROM collection c \
               JOIN shared.printings p ON p.printing_id = c.printing_id \
               JOIN shared.cards cd ON cd.card_id = p.card_id \
              WHERE c.status = 'owned'"
        ))
        .map_err(|e| db_err(e.into()))?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, bool>(7)?,
            ))
        })
        .map_err(|e| db_err(e.into()))?;
    let (mut by_type, mut by_aspect, mut by_set) =
        (Tally::default(), Tally::default(), Tally::default());
    let (mut base_hp, mut base_kind) = (Tally::default(), Tally::default());
    let mut base_kind_aspect: std::collections::BTreeMap<&str, Tally> = Default::default();
    let (mut leader_aspect, mut leader_align) = (CardTally::default(), CardTally::default());
    let mut leader_color_align: std::collections::BTreeMap<String, CardTally> = Default::default();
    for row in rows {
        let (card_id, kind, aspects, hp, epic, rules, set, available) =
            row.map_err(|e| db_err(e.into()))?;
        let kind = kind.unwrap_or_else(|| "Unknown".into());
        let aspects: Vec<String> = serde_json::from_str(&aspects).unwrap_or_default();
        by_type.add(&kind, available);
        by_set.add(&set, available);
        for a in &aspects {
            by_aspect.add(a, available);
        }
        match kind.as_str() {
            "Base" => {
                base_hp.add(&hp.map_or("unknown".into(), |h| h.to_string()), available);
                let has = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.trim().is_empty());
                let kind = if has(&epic) {
                    "epic"
                } else if has(&rules) {
                    "ability"
                } else {
                    "vanilla"
                };
                base_kind.add(kind, available);
                let by_aspect = base_kind_aspect.entry(kind).or_default();
                if aspects.is_empty() {
                    by_aspect.add("none", available);
                }
                for a in &aspects {
                    by_aspect.add(a, available);
                }
            }
            "Leader" => {
                for a in &aspects {
                    leader_aspect.add(a, &card_id, available);
                }
                let align = alignment(&aspects);
                leader_align.add(align, &card_id, available);
                let colors: Vec<&String> = aspects
                    .iter()
                    .filter(|a| !matches!(a.as_str(), "Heroism" | "Villainy"))
                    .collect();
                if colors.is_empty() {
                    leader_color_align
                        .entry("none".into())
                        .or_default()
                        .add(align, &card_id, available);
                }
                for c in colors {
                    leader_color_align
                        .entry(c.clone())
                        .or_default()
                        .add(align, &card_id, available);
                }
            }
            _ => {}
        }
    }
    Ok(json!({
        "by_type": by_type.json(),
        "by_aspect": by_aspect.json(),
        "by_set": by_set.json(),
        "bases": {
            "by_hp": base_hp.json(),
            "by_kind": base_kind.json(),
            "by_kind_aspect": Value::Object(
                base_kind_aspect.iter().map(|(k, t)| (k.to_string(), t.json())).collect()
            ),
        },
        "leaders": {
            "by_aspect": leader_aspect.json(),
            "by_alignment": leader_align.json(),
            "by_aspect_alignment": Value::Object(
                leader_color_align.iter().map(|(k, t)| (k.clone(), t.json())).collect()
            ),
        },
    }))
}

fn structured_for(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, Structured>, ToolError> {
    let mut out = HashMap::new();
    let mut stmt = conn
        .prepare(
            "SELECT keywords, is_unique, epic_action, rules_text FROM cards WHERE card_id = ?1",
        )
        .map_err(|e| db_err(e.into()))?;
    for id in ids {
        let row = stmt
            .query_row([id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })
            .optional()
            .map_err(|e| db_err(e.into()))?;
        if let Some((kw, unique, epic, rules)) = row {
            out.insert(
                id.clone(),
                Structured {
                    keywords: serde_json::from_str(&kw).unwrap_or_else(|_| json!([])),
                    unique: unique != 0,
                    epic_action: epic.filter(|e| !e.is_empty()),
                    rules_text: rules,
                },
            );
        }
    }
    Ok(out)
}

/// The one card-id form the tools speak: `<SET>-<number>` of the card's canonical printing
/// (Standard before other variants, Normal before foil, the earliest set first), e.g. `SOR-5`.
/// Mirrors the ranking `CompiledSearch::set_one_per_card` picks a row by.
const CANONICAL_PRINTING: &str = "ORDER BY variant <> 'Standard', finish <> 'Normal', \
    (SELECT COALESCE(release_date, '9999') FROM shared.sets WHERE set_code = printings.set_code), \
    (SELECT sort_order FROM shared.sets WHERE set_code = printings.set_code), \
    CAST(number AS INTEGER), printing_id LIMIT 1";

fn display_id(conn: &Connection, card_id: &str) -> Result<String, ToolError> {
    let sql = format!(
        "SELECT set_code || '-' || number FROM shared.printings WHERE card_id = ?1 {CANONICAL_PRINTING}"
    );
    Ok(conn
        .query_row(&sql, [card_id], |r| r.get::<_, String>(0))
        .optional()
        .map_err(|e| db_err(e.into()))?
        .unwrap_or_else(|| card_id.to_string()))
}

/// Accepts the documented `<SET>-<number>` and the older internal forms (`n:<slug>`, bare
/// numeric ids) and returns the catalog's own card id.
fn resolve_card_id(conn: &Connection, given: &str) -> Result<String, ToolError> {
    let given = given.trim();
    let known = conn
        .query_row(
            "SELECT card_id FROM shared.cards WHERE card_id = ?1",
            [given],
            |r| r.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| db_err(e.into()))?;
    if let Some(id) = known {
        return Ok(id);
    }
    if let Some((set, number)) = given.split_once('-') {
        let sql = format!(
            "SELECT card_id FROM shared.printings WHERE upper(set_code) = upper(?1) \
               AND (number = ?2 OR ltrim(number, '0') = ltrim(?2, '0')) {CANONICAL_PRINTING}"
        );
        let found = conn
            .query_row(&sql, [set, number], |r| r.get::<_, String>(0))
            .optional()
            .map_err(|e| db_err(e.into()))?;
        if let Some(id) = found {
            return Ok(id);
        }
    }
    Err(failed(format!(
        "no card '{given}'; ids look like SOR-5 (<SET>-<number>), search_cards finds them"
    )))
}

/// The `get_deck` response for deck `id`; `update_deck` echoes the same shape.
pub(crate) fn deck_response(c: &Connection, id: i64) -> Result<Value, ToolError> {
    let deck = decks::get(c, id)
        .map_err(db_err)?
        .ok_or_else(|| failed(format!("not found: deck {id}")))?;
    let lines = decks::line_states(c, id).map_err(db_err)?;
    let mut cards = serde_json::Map::new();
    for l in &lines {
        if cards.contains_key(&display_id(c, &l.card_id)?) {
            continue;
        }
        let found: Option<(String, String, String)> = c
            .query_row(
                &format!("SELECT set_code, number, variant FROM shared.printings WHERE card_id = ?1 {CANONICAL_PRINTING}"),
                [&l.card_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| db_err(e.into()))?;
        let Some((set, number, variant)) = found else {
            continue;
        };
        if let Some(d) =
            cards::get_card_detail_variant(c, &set, &number, Some(&variant)).map_err(db_err)?
        {
            let mut card = detail_card_object(c, &d.card)?;
            card["in_decks"] = json!(
                holders_for(c, std::slice::from_ref(&d.card.card_id))?
                    .get(&d.card.card_id)
                    .cloned()
                    .unwrap_or_default()
            );
            cards.insert(display_id(c, &l.card_id)?, card);
        }
    }
    let lines = display_lines(c, lines)?;
    let cards = Value::Object(cards);
    let lines_digest = crate::digest::lines_digest(&lines, &cards);
    Ok(json!({ "deck": deck, "lines": lines, "lines_digest": lines_digest, "cards": cards }))
}

/// Rewrites every `card_id` in a serialized list of deck lines to the documented form.
fn display_lines(conn: &Connection, lines: impl serde::Serialize) -> Result<Value, ToolError> {
    let mut v = json!(lines);
    for l in v.as_array_mut().into_iter().flatten() {
        if let Some(id) = l["card_id"].as_str().map(String::from) {
            let shown = display_id(conn, &id)?;
            // Prose built from the row names the card as the tools do: id and name.
            if let Some(hint) = l["hint"].as_str().map(String::from) {
                let name: Option<String> = conn
                    .query_row(
                        "SELECT name FROM shared.cards WHERE card_id = ?1",
                        [&id],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(|e| db_err(e.into()))?;
                let label = match name {
                    Some(n) => format!("{shown} {n}"),
                    None => shown.clone(),
                };
                l["hint"] = json!(hint.replace(&id, &label));
            }
            l["card_id"] = json!(shown);
        }
    }
    Ok(v)
}

/// The card object: the common fields plus a `game` block.
fn card_object(
    f: Facts,
    counts: &HashMap<String, CardCount>,
    structured: &HashMap<String, Structured>,
) -> Value {
    let (keywords, unique, epic_action, stored_text) = match structured.get(f.card_id) {
        Some(s) => (
            s.keywords.clone(),
            json!(s.unique),
            json!(s.epic_action),
            s.rules_text.as_deref(),
        ),
        None => (json!([]), json!(false), Value::Null, None),
    };
    let rules_text = f.rules_text.or(stored_text);
    let aspects = json_array(f.aspects);
    let (owned, available) = counts
        .get(f.card_id)
        .map(|c| (c.owned, c.available))
        .unwrap_or((0, 0));
    // The card's set and number are those of its canonical printing, the ones its id
    // names, whichever printing the search matched (sp-wxj6lr).
    let (set, number) = match f.display_id.split_once('-') {
        Some((s, n)) if f.display_id != f.card_id => (s, n),
        _ => (f.set, f.number),
    };
    json!({
        "id": f.display_id,
        "name": f.name,
        "subtitle": f.subtitle,
        "set": set,
        "number": number,
        "rarity": f.rarity,
        "type": f.card_type,
        "cost": f.cost,
        "owned_count": owned,
        "available_count": available,
        "game": {
            "aspects": aspects,
            "alignment": alignment(&aspects),
            "arenas": json_array(f.arenas),
            "power": f.power,
            "hp": f.hp,
            "traits": json_array(f.traits),
            "keywords": keywords,
            "unique": unique,
            "epic_action": epic_action,
            "rules_text": rules_text,
        },
    })
}

fn row_object(
    conn: &Connection,
    r: &SearchRow,
    counts: &HashMap<String, CardCount>,
    structured: &HashMap<String, Structured>,
    holds: &HashMap<String, Vec<decks::DeckHold>>,
    expand: bool,
) -> Result<Value, ToolError> {
    let mut v = card_object(
        Facts {
            card_id: &r.card_id,
            display_id: display_id(conn, &r.card_id)?,
            name: &r.name,
            subtitle: &r.subtitle,
            set: &r.set_code,
            number: &r.number,
            rarity: &r.rarity,
            card_type: &r.card_type,
            cost: r.cost,
            power: r.power,
            hp: r.hp,
            aspects: &r.aspects,
            arenas: &r.arenas,
            traits: &r.traits,
            rules_text: None,
        },
        counts,
        structured,
    );
    v["in_decks"] = json!(holds.get(&r.card_id).cloned().unwrap_or_default());
    if expand {
        v["printing_id"] = json!(r.printing_id);
        v["printing_set"] = json!(r.set_code);
        v["printing_number"] = json!(r.number);
        v["variant"] = json!(r.variant);
        v["finish"] = json!(r.finish);
        v["owned_printing_count"] = json!(r.owned_count);
    }
    Ok(v)
}

/// `in_decks: [{id, name, qty}]` for each card, set on the card objects.
fn holders_for(
    conn: &Connection,
    ids: &[String],
) -> Result<HashMap<String, Vec<decks::DeckHold>>, ToolError> {
    decks::holders(conn, ids).map_err(db_err)
}

fn counts_for(conn: &Connection, ids: &[String]) -> Result<HashMap<String, CardCount>, ToolError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(decks::card_counts(conn, Some(ids))
        .map_err(db_err)?
        .into_iter()
        .map(|c| (c.card_id.clone(), c))
        .collect())
}

/// A parse error that names the nearest valid terms instead of the bare refusal.
fn teach(keywords: &Keywords, message: &str) -> String {
    let unknown = message
        .strip_prefix("unknown keyword: '")
        .and_then(|r| r.strip_suffix('\''));
    match unknown {
        Some(term) => keywords.unknown_term_message(term.trim_end_matches([':', '=', '<', '>'])),
        None => message.to_string(),
    }
}

fn schema(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

/// The card object of one card, as `get_card` and `get_deck` return it.
fn detail_card_object(c: &Connection, card: &Card) -> Result<Value, ToolError> {
    let Card {
        card_id,
        set_code,
        number,
        name,
        subtitle,
        card_type,
        cost,
        power,
        hp,
        aspects,
        traits,
        arenas,
        rules_text,
        rarity,
        ..
    } = card;
    let counts = counts_for(c, std::slice::from_ref(card_id))?;
    let structured = structured_for(c, std::slice::from_ref(card_id))?;
    Ok(card_object(
        Facts {
            card_id,
            display_id: display_id(c, card_id)?,
            name,
            subtitle,
            set: set_code,
            number,
            rarity,
            card_type,
            cost: *cost,
            power: *power,
            hp: *hp,
            aspects,
            arenas,
            traits,
            rules_text: rules_text.as_deref(),
        },
        &counts,
        &structured,
    ))
}

/// The `search_cards` result for `args`; `/mcp` and `/api/agent/search` both serve it.
pub(crate) fn search_cards_value(
    c: &mut Connection,
    registry: &KeywordRegistry,
    flags: &[SearchFlag],
    keywords: &Keywords,
    args: &Value,
) -> Result<Value, ToolError> {
    let q = args.get("q").and_then(Value::as_str).unwrap_or("").trim();
    let limit = match args.get("limit").and_then(Value::as_u64) {
        None => DEFAULT_PAGE,
        Some(n) if (1..=MAX_PAGE as u64).contains(&n) => n as u32,
        Some(_) => return Err(failed(format!("limit must be between 1 and {MAX_PAGE}"))),
    };
    let mut compiled = if q.is_empty() {
        search::compile_all()
    } else {
        let ast = parse(q, registry).map_err(|e| {
            failed(format!(
                "{} (at position {})",
                teach(keywords, &e.message),
                e.position
            ))
        })?;
        search::compile(&ast, flags)
    };
    let owned_only = args.get("owned_only").and_then(Value::as_bool) == Some(true);
    compiled.set_catalog_wide(!owned_only);
    let expand = args.get("expand_printings").and_then(Value::as_bool) == Some(true);
    if !expand {
        compiled.set_one_per_card();
    }
    let cursor = arg_str(args, "cursor").map(String::from);
    let page =
        search::search_page(c, &compiled, Slice::Cursor { limit, cursor }).map_err(db_err)?;
    let ids: Vec<String> = page.rows.iter().map(|r| r.card_id.clone()).collect();
    let counts = counts_for(c, &ids)?;
    let structured = structured_for(c, &ids)?;
    let holds = holders_for(c, &ids)?;
    let (copies_owned, copies_available, copies_reserved) =
        search::copy_totals(c, &compiled, !expand).map_err(db_err)?;
    Ok(json!({
        "results": page.rows.iter().map(|r| row_object(c, r, &counts, &structured, &holds, expand)).collect::<Result<Vec<_>, _>>()?,
        "total": page.total,
        "total_copies_owned": copies_owned,
        "total_copies_available": copies_available,
        "total_copies_reserved": copies_reserved,
        "next_cursor": page.next_cursor,
    }))
}

impl DumpstarTools {
    fn tenant_writer(
        &self,
        ctx: &ToolContext,
    ) -> Result<(TenantId, Arc<Mutex<Connection>>), ToolError> {
        let id = self
            .ids
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ctx.tenant)
            .cloned()
            .ok_or_else(|| ToolError::Internal("tenant was not resolved".into()))?;
        let conn = self
            .tenants
            .connection(&id)
            .map_err(|e| ToolError::Internal(e.to_string()))?;
        Ok((id, conn))
    }

    /// A write: takes the tenant's one writer connection, so writes stay serialised.
    fn with_conn<T>(
        &self,
        ctx: &ToolContext,
        tool: &str,
        f: impl FnOnce(&mut Connection) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let (_, conn) = self.tenant_writer(ctx)?;
        let mut guard = conn.lock().unwrap_or_else(|e| e.into_inner());
        dbpool::bounded(&mut guard, tool, f, ToolError::Failed)
    }

    /// A read: its own read-only connection, so it neither waits for nor blocks any other call.
    fn with_read<T>(
        &self,
        ctx: &ToolContext,
        tool: &str,
        f: impl FnOnce(&mut Connection) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
        let (id, writer) = self.tenant_writer(ctx)?;
        let mut reader = dbpool::checkout(&id, &writer).map_err(db_err)?;
        dbpool::bounded(&mut reader, tool, f, ToolError::Failed)
    }

    fn search_cards(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        self.with_read(ctx, "search_cards", |c| {
            search_cards_value(c, &self.registry, &self.flags, &self.keywords, args)
        })
    }

    fn get_card(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        self.with_read(ctx, "get_card", |c| {
            let (set, number, variant) = match arg_str(args, "id") {
                Some(given) => {
                    let card_id = resolve_card_id(c, given)?;
                    let card_id = card_id.as_str();
                    c.query_row(
                        &format!("SELECT set_code, number, variant FROM shared.printings WHERE card_id = ?1 {CANONICAL_PRINTING}"),
                        [card_id],
                        |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, String>(1)?,
                                Some(r.get::<_, String>(2)?),
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| db_err(e.into()))?
                    .ok_or_else(|| failed(format!("no card '{card_id}'; search_cards finds ids")))?
                }
                None => match (arg_str(args, "set"), arg_str(args, "number")) {
                    (Some(s), Some(n)) => (
                        s.to_string(),
                        n.to_string(),
                        arg_str(args, "variant").map(String::from),
                    ),
                    _ => return Err(failed("give 'id', or 'set' and 'number'")),
                },
            };
            let detail = cards::get_card_detail_variant(c, &set, &number, variant.as_deref())
                .map_err(db_err)?
                .ok_or_else(|| failed(format!("no card printed as {set}/{number}")))?;
            let mut card = detail_card_object(c, &detail.card)?;
            card["in_decks"] = json!(holders_for(c, std::slice::from_ref(&detail.card.card_id))?
                .remove(&detail.card.card_id)
                .unwrap_or_default());
            Ok(json!({
                "card": card,
                "printings": detail.printings.iter().map(|p| json!({
                    "printing_id": p.printing_id, "set": p.set_code, "number": p.number,
                    "variant": p.variant, "finish": p.finish, "rarity": p.rarity,
                    "owned_count": p.owned_count, "market_price": p.market_price,
                })).collect::<Vec<_>>(),
                "copies": detail.copies.iter().map(|e| json!({
                    "copy_id": e.id, "printing_id": e.printing_id, "condition": e.condition,
                    "language": e.language, "status": e.status, "deck_id": e.deck_id,
                    "binder_id": e.binder_id,
                })).collect::<Vec<_>>(),
            }))
        })
    }

    fn collection_summary(&self, ctx: &ToolContext) -> Result<Value, ToolError> {
        self.with_read(ctx, "collection_summary", |c| {
            let counts = decks::card_counts(c, None).map_err(db_err)?;
            let decks = decks::list(c).map_err(db_err)?;
            let mut groups: Vec<&str> = decks.iter().filter_map(|d| d.group.as_deref()).collect();
            groups.sort();
            groups.dedup();
            Ok(json!({
                "owned_copies": counts.iter().map(|c| c.owned as u64).sum::<u64>(),
                "reserved_copies": counts.iter().map(|c| c.reserved as u64).sum::<u64>(),
                "available_copies": counts.iter().map(|c| c.available as u64).sum::<u64>(),
                "distinct_cards": counts.len(),
                "decks": decks.len(),
                "deck_groups": groups,
                "breakdown": breakdown(c)?,
            }))
        })
    }

    fn list_decks(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let (group, format) = (arg_str(args, "group"), arg_str(args, "format"));
        self.with_read(ctx, "list_decks", |c| {
            let decks: Vec<_> = decks::list(c)
                .map_err(db_err)?
                .into_iter()
                .filter(|d| group.is_none_or(|g| d.group.as_deref() == Some(g)))
                .filter(|d| format.is_none_or(|f| d.format.as_deref() == Some(f)))
                .collect();
            Ok(json!({ "decks": decks }))
        })
    }

    fn get_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_read(ctx, "get_deck", |c| deck_response(c, id))
    }

    fn create_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let name = arg_str(args, "name").ok_or_else(|| failed("'name' is required"))?;
        let new = NewDeck {
            name: name.to_string(),
            format: arg_str(args, "format").map(String::from),
            group: arg_str(args, "group").map(String::from),
            description: arg_str(args, "description").map(String::from),
            notes: arg_str(args, "notes").map(String::from),
            ..Default::default()
        };
        self.with_conn(ctx, "create_deck", |c| {
            let id = decks::create(c, &new).map_err(db_err)?;
            let deck = decks::get(c, id).map_err(db_err)?;
            Ok(json!({ "deck": deck }))
        })
    }

    fn update_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        const KEYS: [&str; 6] = ["id", "name", "format", "group", "description", "notes"];
        let obj = args
            .as_object()
            .ok_or_else(|| failed("arguments must be an object"))?;
        if let Some(k) = obj.keys().find(|k| !KEYS.contains(&k.as_str())) {
            return Err(failed(format!(
                "unknown key '{k}'; update_deck takes id, name, format, group, description, notes"
            )));
        }
        let id = arg_id(args, "id")?;
        // Absent leaves the field; null or "" clears it.
        let field = |k: &str| -> Result<Option<Option<String>>, ToolError> {
            match obj.get(k) {
                None => Ok(None),
                Some(Value::Null) => Ok(Some(None)),
                Some(Value::String(s)) if s.is_empty() => Ok(Some(None)),
                Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
                Some(_) => Err(failed(format!("'{k}' must be a string or null"))),
            }
        };
        let name = match obj.get("name") {
            None => None,
            Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
            Some(_) => return Err(failed("'name' must be a non-empty string")),
        };
        let patch = decks::DeckPatch {
            name,
            description: field("description")?,
            format: field("format")?,
            group: field("group")?,
            notes: field("notes")?,
        };
        self.with_conn(ctx, "update_deck", |c| {
            if !decks::patch(c, id, &patch).map_err(db_err)? {
                return Err(failed(format!("not found: deck {id}")));
            }
            deck_response(c, id)
        })
    }

    fn set_deck_lines(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        let raw = args
            .get("lines")
            .and_then(Value::as_array)
            .ok_or_else(|| failed("'lines' must be an array"))?;
        let mut lines = Vec::with_capacity(raw.len());
        for l in raw {
            let card_id =
                arg_str(l, "card_id").ok_or_else(|| failed("each line needs 'card_id'"))?;
            let qty = l
                .get("qty")
                .and_then(Value::as_u64)
                .and_then(|q| u32::try_from(q).ok())
                .ok_or_else(|| {
                    failed(format!("line for '{card_id}' needs a whole-number 'qty'"))
                })?;
            let zone = match arg_str(l, "zone") {
                None | Some("main") => Zone::Main,
                Some("sideboard") => Zone::Sideboard,
                Some(z) => {
                    return Err(failed(format!("zone must be main or sideboard, not '{z}'")));
                }
            };
            lines.push(DeckEntry {
                card_id: card_id.to_string(),
                qty,
                zone,
            });
        }
        self.with_conn(ctx, "set_deck_lines", |c| {
            for l in &mut lines {
                l.card_id = resolve_card_id(c, &l.card_id)?;
            }
            match decks::set_lines(c, id, &lines).map_err(db_err)? {
                SetLines::Short(short) => Err(failed(format!(
                    "not enough free copies; nothing changed. short lines: {}",
                    display_lines(c, short)?
                ))),
                SetLines::Set { reserved } => Ok(json!({
                    "reserved": reserved,
                    "lines": display_lines(c, decks::line_states(c, id).map_err(db_err)?)?,
                })),
            }
        })
    }

    fn release_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_conn(ctx, "release_deck", |c| {
            Ok(json!({ "released": decks::release(c, id).map_err(db_err)? }))
        })
    }

    fn delete_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_conn(ctx, "delete_deck", |c| {
            if decks::delete(c, id).map_err(db_err)? {
                Ok(json!({ "deleted": id }))
            } else {
                Err(failed(format!("not found: deck {id}")))
            }
        })
    }

    fn validate_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_read(ctx, "validate_deck", |c| {
            let v = decks::validate(c, id).map_err(db_err)?;
            Ok(json!({
                "sanctioned": v.format_id.is_some(),
                "format_id": v.format_id,
                "report": v.report,
                "note": if v.format_id.is_none() {
                    "The deck's format label names no sanctioned format, so no rules are checked."
                } else { "" },
            }))
        })
    }
}

impl ToolProvider for DumpstarTools {
    fn tools(&self) -> Vec<ToolDef> {
        let def = |name: &str, description: String, input_schema: Value| ToolDef {
            name: name.into(),
            description,
            input_schema,
        };
        let id = json!({"type": "integer", "description": "The deck's id"});
        vec![
            def(
                "search_cards",
                self.keywords.tool_description(
                    "Search the Star Wars: Unlimited catalog with the site's query language. \
                     Each result is one card (its original printing); owned_count and available_count \
                     are across all its printings (copies owned, and owned copies in no deck). \
                     Pass expand_printings for one row per printing instead. Page with next_cursor. Every result's id is a card id of the form <SET>-<number>, e.g. SOR-5; every tool takes and returns that form.",
                    self.keywords.0.len(),
                    &EXAMPLES,
                ),
                schema(
                    json!({
                        "q": {"type": "string", "description": "The query; empty matches everything"},
                        "cursor": {"type": "string", "description": "next_cursor of the previous page"},
                        "limit": {"type": "integer", "description": format!("Rows per page, 1 to {MAX_PAGE}; default {DEFAULT_PAGE}")},
                        "owned_only": {"type": "boolean", "description": "Only cards the user owns; default false (whole catalog)"},
                        "expand_printings": {"type": "boolean", "description": "One row per printing, with printing_id, variant, finish and owned_printing_count; default false (one row per card)"},
                    }),
                    &["q"],
                ),
            ),
            def(
                "get_card",
                "One card in full: its game block and rules text, its printings with owned counts, and the user's copies with the deck each sits in. Give id (a card id, e.g. SOR-5) or set and number.".into(),
                schema(
                    json!({
                        "id": {"type": "string", "description": CARD_ID_DOC},
                        "set": {"type": "string", "description": "Set code, e.g. SOR"},
                        "number": {"type": "string", "description": "Collector number"},
                        "variant": {"type": "string", "description": "Standard, Hyperspace, Showcase or Promo"},
                    }),
                    &[],
                ),
            ),
            def(
                "query_help",
                "The search vocabulary in full: every term with its aliases, operators and help.".into(),
                schema(json!({}), &[]),
            ),
            def(
                "collection_summary",
                "Counts of owned, reserved (in a deck) and available copies, distinct cards, decks and deck groups, with a `breakdown` of owned and available copies by type, aspect and set; bases by HP and epic or vanilla; leaders by aspect and alignment. `leaders.by_aspect` is a raw count of leaders having each aspect: a two-color leader counts under both colors and a leader with no color under none, so it overstates the leaders that can lead a pack of one color; `leaders.by_aspect_alignment` splits each color by alignment.".into(),
                schema(json!({}), &[]),
            ),
            def(
                "list_decks",
                "The user's decks with format label, group and card count, optionally of one group or format label.".into(),
                schema(
                    json!({
                        "group": {"type": "string"},
                        "format": {"type": "string"},
                    }),
                    &[],
                ),
            ),
            def(
                "get_deck",
                "One deck and its lines, each with the copies it reserves, and under cards the full card object of every line keyed by card id (as get_card returns it), so the result is a self-contained pack document. `lines_digest` is the SHA-256 (lowercase hex) of the lines and their card objects in canonical form: one row per line, sorted, of `card_id<TAB>qty<TAB>reserved` then the card's name, subtitle, type, rarity, cost, game.hp, game.epic_action, game.aspects and game.arenas as compact JSON (arrays sorted), each row ending in LF (zone is not included; the field list is DIGEST_CARD_FIELDS in src/digest.rs); a client recomputes it and refuses a file whose lines or cards were altered in copying.".into(),
                schema(json!({"id": id}), &["id"]),
            ),
            def(
                "create_deck",
                "Create an empty deck. A format label that names a sanctioned format (Premier, Twin Suns) is checked by validate_deck; any other label is the user's own and is not enforced. group collects decks, such as the packs of a cube.".into(),
                schema(
                    json!({
                        "name": {"type": "string"},
                        "format": {"type": "string", "description": "Free-text format label"},
                        "group": {"type": "string", "description": "Optional group name"},
                        "description": {"type": "string"},
                        "notes": {"type": "string"},
                    }),
                    &["name", "format"],
                ),
            ),
            def(
                "update_deck",
                "Change a deck's own fields, keeping its id and lines: rename it, move it to another group, relabel its format, edit description or notes. Partial: an absent key is left unchanged, and null (or an empty string) clears an optional field; name cannot be cleared. Returns exactly what get_deck returns for the updated deck ({deck, lines, lines_digest, cards}). Rename or regroup with this, never delete and recreate.".into(),
                schema(
                    json!({
                        "id": id,
                        "name": {"type": "string"},
                        "format": {"type": ["string", "null"], "description": "Free-text format label"},
                        "group": {"type": ["string", "null"], "description": "Group name"},
                        "description": {"type": ["string", "null"]},
                        "notes": {"type": ["string", "null"]},
                    }),
                    &["id"],
                ),
            ),
            def(
                "set_deck_lines",
                "Replace the deck's lines in one transaction and reserve an owned copy for every card. A copy in another deck is never taken. If any card lacks enough free copies nothing changes and the short lines are returned.".into(),
                schema(
                    json!({
                        "id": id,
                        "lines": {"type": "array", "description": "Objects {card_id, qty, zone?} with zone main (default) or sideboard", "items": {
                            "type": "object",
                            "properties": {
                                "card_id": {"type": "string", "description": CARD_ID_DOC},
                                "qty": {"type": "integer"},
                                "zone": {"type": "string", "enum": ["main", "sideboard"]},
                            },
                            "required": ["card_id", "qty"],
                        }},
                    }),
                    &["id", "lines"],
                ),
            ),
            def(
                "release_deck",
                "Free the deck's copies for other decks; its lines stay.".into(),
                schema(json!({"id": id}), &["id"]),
            ),
            def(
                "delete_deck",
                "Delete the deck. Its copies are freed, never deleted.".into(),
                schema(json!({"id": id}), &["id"]),
            ),
            def(
                "validate_deck",
                "Check the deck against its sanctioned format's rules. A deck whose format label is the user's own is not checked.".into(),
                schema(json!({"id": id}), &["id"]),
            ),
        ]
    }

    fn call(&self, ctx: &ToolContext, name: &str, args: Value) -> Result<Value, ToolError> {
        match name {
            "search_cards" => self.search_cards(ctx, &args),
            "get_card" => self.get_card(ctx, &args),
            "query_help" => Ok(self.keywords.help_value()),
            "collection_summary" => self.collection_summary(ctx),
            "list_decks" => self.list_decks(ctx, &args),
            "get_deck" => self.get_deck(ctx, &args),
            "create_deck" => self.create_deck(ctx, &args),
            "update_deck" => self.update_deck(ctx, &args),
            "set_deck_lines" => self.set_deck_lines(ctx, &args),
            "release_deck" => self.release_deck(ctx, &args),
            "delete_deck" => self.delete_deck(ctx, &args),
            "validate_deck" => self.validate_deck(ctx, &args),
            other => Err(failed(format!("unknown tool '{other}'"))),
        }
    }
}
