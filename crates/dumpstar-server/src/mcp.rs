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
    let keywords = Keywords::parse(dumpstar_db::search_meta::keywords_json())?;
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

/// Heroism is Heroic, Villainy is Villainous, anything else is Neutral.
fn alignment(aspects: &[String]) -> &'static str {
    if aspects.iter().any(|a| a == "Heroism") {
        "Heroic"
    } else if aspects.iter().any(|a| a == "Villainy") {
        "Villainous"
    } else {
        "Neutral"
    }
}

struct Facts<'a> {
    card_id: &'a str,
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
    json!({
        "id": f.card_id,
        "name": f.name,
        "subtitle": f.subtitle,
        "set": f.set,
        "number": f.number,
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
    r: &SearchRow,
    counts: &HashMap<String, CardCount>,
    structured: &HashMap<String, Structured>,
) -> Value {
    let mut v = card_object(
        Facts {
            card_id: &r.card_id,
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
    v["printing_id"] = json!(r.printing_id);
    v["variant"] = json!(r.variant);
    v["finish"] = json!(r.finish);
    v["owned_printing_count"] = json!(r.owned_count);
    v
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

impl DumpstarTools {
    fn with_conn<T>(
        &self,
        ctx: &ToolContext,
        f: impl FnOnce(&mut Connection) -> Result<T, ToolError>,
    ) -> Result<T, ToolError> {
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
        let mut guard = conn.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut guard)
    }

    fn search_cards(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let q = args.get("q").and_then(Value::as_str).unwrap_or("").trim();
        let limit = match args.get("limit").and_then(Value::as_u64) {
            None => DEFAULT_PAGE,
            Some(n) if (1..=MAX_PAGE as u64).contains(&n) => n as u32,
            Some(_) => return Err(failed(format!("limit must be between 1 and {MAX_PAGE}"))),
        };
        let mut compiled = if q.is_empty() {
            search::compile_all()
        } else {
            let ast = parse(q, &self.registry).map_err(|e| {
                failed(format!(
                    "{} (at position {})",
                    teach(&self.keywords, &e.message),
                    e.position
                ))
            })?;
            search::compile(&ast, &self.flags)
        };
        let owned_only = args.get("owned_only").and_then(Value::as_bool) == Some(true);
        compiled.set_catalog_wide(!owned_only);
        let cursor = arg_str(args, "cursor").map(String::from);
        self.with_conn(ctx, |c| {
            let page = search::search_page(c, &compiled, Slice::Cursor { limit, cursor })
                .map_err(db_err)?;
            let ids: Vec<String> = page.rows.iter().map(|r| r.card_id.clone()).collect();
            let counts = counts_for(c, &ids)?;
            let structured = structured_for(c, &ids)?;
            Ok(json!({
                "results": page.rows.iter().map(|r| row_object(r, &counts, &structured)).collect::<Vec<_>>(),
                "total": page.total,
                "next_cursor": page.next_cursor,
            }))
        })
    }

    fn get_card(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        self.with_conn(ctx, |c| {
            let (set, number, variant) = match arg_str(args, "id") {
                Some(card_id) => {
                    c.query_row(
                        "SELECT set_code, number, variant FROM shared.printings WHERE card_id = ?1 \
                          ORDER BY variant <> 'Standard', finish <> 'Normal', printing_id LIMIT 1",
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
            let card = detail_card_object(c, &detail.card)?;
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
        self.with_conn(ctx, |c| {
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
            }))
        })
    }

    fn list_decks(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let (group, format) = (arg_str(args, "group"), arg_str(args, "format"));
        self.with_conn(ctx, |c| {
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
        self.with_conn(ctx, |c| {
            let deck = decks::get(c, id)
                .map_err(db_err)?
                .ok_or_else(|| failed(format!("not found: deck {id}")))?;
            let lines = decks::line_states(c, id).map_err(db_err)?;
            let mut cards = serde_json::Map::new();
            for l in &lines {
                if cards.contains_key(&l.card_id) {
                    continue;
                }
                let found: Option<(String, String, String)> = c
                    .query_row(
                        "SELECT set_code, number, variant FROM shared.printings WHERE card_id = ?1 \
                          ORDER BY variant <> 'Standard', finish <> 'Normal', printing_id LIMIT 1",
                        [&l.card_id],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()
                    .map_err(|e| db_err(e.into()))?;
                let Some((set, number, variant)) = found else {
                    continue;
                };
                if let Some(d) = cards::get_card_detail_variant(c, &set, &number, Some(&variant))
                    .map_err(db_err)?
                {
                    cards.insert(l.card_id.clone(), detail_card_object(c, &d.card)?);
                }
            }
            Ok(json!({ "deck": deck, "lines": lines, "cards": cards }))
        })
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
        self.with_conn(ctx, |c| {
            let id = decks::create(c, &new).map_err(db_err)?;
            let deck = decks::get(c, id).map_err(db_err)?;
            Ok(json!({ "deck": deck }))
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
        self.with_conn(ctx, |c| {
            match decks::set_lines(c, id, &lines).map_err(db_err)? {
                SetLines::Short(short) => Err(failed(format!(
                    "not enough free copies; nothing changed. short lines: {}",
                    json!(short)
                ))),
                SetLines::Set { reserved } => Ok(json!({
                    "reserved": reserved,
                    "lines": decks::line_states(c, id).map_err(db_err)?,
                })),
            }
        })
    }

    fn release_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_conn(ctx, |c| {
            Ok(json!({ "released": decks::release(c, id).map_err(db_err)? }))
        })
    }

    fn delete_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_conn(ctx, |c| {
            if decks::delete(c, id).map_err(db_err)? {
                Ok(json!({ "deleted": id }))
            } else {
                Err(failed(format!("not found: deck {id}")))
            }
        })
    }

    fn validate_deck(&self, ctx: &ToolContext, args: &Value) -> Result<Value, ToolError> {
        let id = arg_id(args, "id")?;
        self.with_conn(ctx, |c| {
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
                     Each result is one printing; owned_count and available_count are per card \
                     (copies owned, and owned copies in no deck). Page with next_cursor.",
                    10,
                    &EXAMPLES,
                ),
                schema(
                    json!({
                        "q": {"type": "string", "description": "The query; empty matches everything"},
                        "cursor": {"type": "string", "description": "next_cursor of the previous page"},
                        "limit": {"type": "integer", "description": format!("Rows per page, 1 to {MAX_PAGE}; default {DEFAULT_PAGE}")},
                        "owned_only": {"type": "boolean", "description": "Only printings the user owns; default false (whole catalog)"},
                    }),
                    &["q"],
                ),
            ),
            def(
                "get_card",
                "One card in full: its game block and rules text, its printings with owned counts, and the user's copies with the deck each sits in. Give id (a card id from search_cards) or set and number.".into(),
                schema(
                    json!({
                        "id": {"type": "string", "description": "Card id"},
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
                "Counts of owned, reserved (in a deck) and available copies, distinct cards, decks and deck groups.".into(),
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
                "One deck and its lines, each with the copies it reserves, and under cards the full card object of every line keyed by card id (as get_card returns it), so the result is a self-contained pack document.".into(),
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
                "set_deck_lines",
                "Replace the deck's lines in one transaction and reserve an owned copy for every card. A copy in another deck is never taken. If any card lacks enough free copies nothing changes and the short lines are returned.".into(),
                schema(
                    json!({
                        "id": id,
                        "lines": {"type": "array", "description": "Objects {card_id, qty, zone?} with zone main (default) or sideboard", "items": {
                            "type": "object",
                            "properties": {
                                "card_id": {"type": "string"},
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
            "set_deck_lines" => self.set_deck_lines(ctx, &args),
            "release_deck" => self.release_deck(ctx, &args),
            "delete_deck" => self.delete_deck(ctx, &args),
            "validate_deck" => self.validate_deck(ctx, &args),
            other => Err(failed(format!("unknown tool '{other}'"))),
        }
    }
}
