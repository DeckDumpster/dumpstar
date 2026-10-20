//! SQL compiler for the collection search query language.
//!
//! Walks the [`Ast`] produced by `dumpstar_core::query` and emits a
//! parameterized WHERE clause, then assembles a full per-printing query.
//! See architecture/SEARCH_QUERY_LANGUAGE.md §5–7.
//!
//! Design highlights:
//! - **One row per printing** in both modes (decision D3). Collection-row
//!   predicates compile to `EXISTS` over the printing's copies; `owned_count`
//!   is a scalar subquery; `qty:` counts copies of the same printing (D4).
//! - **Two templates**: owned mode (default — requires an owned/ordered copy
//!   unless an explicit `status:` is present) and catalog-wide mode (triggered
//!   by `is:missing`/`is:unowned`/`is:owned`, surfacing unowned printings).
//! - **No value interpolation** — every user value is a bound parameter.
//! - **`1=0` sentinel** for a keyword/flag that parses but can't compile, so
//!   the corpus/generative tests can assert it never appears for supported
//!   queries.

use std::collections::HashMap;

use dumpstar_core::query::{Ast, Op};
use rusqlite::types::Value;
use rusqlite::{Connection, params_from_iter};

use crate::error::Result;
use crate::search_meta::SearchFlag;

// Scalar subqueries reused across the SELECT list and predicates.
const OWNED_COUNT_SUBQ: &str =
    "SELECT COUNT(*) FROM collection c WHERE c.printing_id = p.printing_id";
// What this printing's OWNED copies are worth: its market price times the sum
// of its owned copies' condition multipliers. The per-row half of
// `SearchPage::total_value`; see that field for why `status = 'owned'` and not
// the wider set owned mode lists.
//
// The price is drawn ONCE rather than per copy inside the aggregate. A
// printing's market price depends only on `p` (that is what pd-m4gw's rule
// bought), so price x SUM(multiplier) is the same number as SUM(price x
// multiplier), and the three-arm COALESCE is then evaluated once per printing
// instead of once per owned copy.
//
// Every arm of it is nullable, and that is load-bearing: a printing nobody
// prices, or one with no owned copy at all, answers NULL, which the outer
// `SUM` ignores. That is what makes a result with nothing owned and priced in
// it `None` rather than `$0.00`.
//
// The `CASE` is not decoration. Catalog-wide mode matches one row per printing
// in the catalog (56k on the real one) and owns a couple of thousand of them,
// so without it the three-arm price lookup runs ~54,000 times to be multiplied
// by NULL. `CASE` short-circuits in SQLite, so the price is drawn only for a
// printing something is actually owned of, and every other row costs one index
// probe.
const OWNED_VALUE_SUBQ: &str = concat!(
    "CASE WHEN EXISTS (SELECT 1 FROM collection c \
                        WHERE c.printing_id = p.printing_id AND c.status = 'owned') \
          THEN (",
    crate::market_price_expr!(),
    ") * (SELECT SUM(COALESCE(cond.multiplier, 1.0)) \
            FROM collection c \
            LEFT JOIN conditions cond ON cond.name = c.condition \
           WHERE c.printing_id = p.printing_id AND c.status = 'owned') \
     END"
);
// One rule for "what is this printing worth", defined in `crate::prices` and
// spent by every surface that draws or orders by a price. For a catalog
// printing it resolves entirely inside `shared`, which is what lets
// `ORDER BY price` stop spanning the ATTACH boundary (pd-m4gw).
use crate::prices::MARKET_PRICE_EXPR;
// printings ⋃ user_printings (`printing_rel!`), joined to cards + sets. Mirrors
// collection::ROW_FROM.
const FROM_CLAUSE: &str = concat!(
    "FROM ",
    crate::printing_rel!(),
    " p \
     JOIN cards cd ON p.card_id = cd.card_id \
     JOIN sets s ON p.set_code = s.set_code"
);

// A collector number as a sortable integer: plain numbers by value, lettered
// ones (P01, T02) after every plain one. The SQL twin of
// `dumpstar_core::number_sort_key`.
const NUMBER_KEY: &str = "CASE WHEN p.number GLOB '*[^0-9]*' \
     THEN 1000000 + COALESCE(CAST(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(REPLACE(\
         p.number, 'P', ''), 'T', ''), 'p', ''), 't', ''), '/', ''), ' ', '') AS INTEGER), 0) \
     ELSE CAST(p.number AS INTEGER) END";

/// A per-copy summary nested under a [`SearchRow`] in the expand/drill-down.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct CopySummary {
    #[ts(type = "number")]
    pub id: i64,
    pub condition: String,
    pub language: String,
    pub status: String,
    pub graded: bool,
    pub purchase_price: Option<f64>,
    pub acquired_at: String,
    #[ts(type = "number | null")]
    pub binder_id: Option<i64>,
    #[ts(type = "number | null")]
    pub deck_id: Option<i64>,
}

/// One search result row — a single printing, owned or not (decision D3).
///
/// **Every field here is one the search list actually draws.** A field nobody
/// renders costs its bytes *and* its key name once per row (pd-lk8v) — which
/// was ~57k times over before the endpoint answered in pages (pd-jsby), and is
/// still the whole page. Anything only the card modal shows belongs on `CardDetail`,
/// which is fetched for the one card the user clicked — see
/// `list_payload_carries_only_what_the_list_renders`, which fails if a field
/// is added here without that decision being made.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchRow {
    pub printing_id: String,
    pub card_id: String,
    pub set_code: String,
    pub set_name: String,
    pub number: String,
    pub name: String,
    pub subtitle: Option<String>,
    pub rarity: Option<String>,
    /// `Leader` | `Unit` | `Event` | `Upgrade` | `Base` | `Token`.
    pub card_type: Option<String>,
    #[ts(type = "number | null")]
    pub cost: Option<i64>,
    #[ts(type = "number | null")]
    pub power: Option<i64>,
    #[ts(type = "number | null")]
    pub hp: Option<i64>,
    /// JSON array, e.g. `["Vigilance","Heroism"]`.
    pub aspects: Option<String>,
    /// JSON array, e.g. `["Ground"]`.
    pub arenas: Option<String>,
    /// JSON array, e.g. `["Underworld","Droid","Bounty Hunter"]`.
    pub traits: Option<String>,
    pub market_price: Option<f64>,
    pub image_url: Option<String>,
    /// The deployed side of a leader printing; `None` for one-sided cards.
    pub back_image_url: Option<String>,
    pub variant: String,
    /// `Normal` or `Foil`.
    pub finish: String,
    /// True when at least one copy is owned (`owned_count > 0`).
    pub owned: bool,
    #[ts(type = "number")]
    pub owned_count: i64,
    /// The owned copies of this printing (empty when unowned).
    pub copies: Vec<CopySummary>,
}

/// One page of search results plus the size of the whole result set.
#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct SearchPage {
    pub rows: Vec<SearchRow>,
    /// Rows the query matches in total, ignoring `limit`/`offset`.
    #[ts(type = "number")]
    pub total: i64,
    /// [`SearchPage::total`] in money: the condition-adjusted market value of
    /// every **owned** copy of every printing the query matches, ignoring
    /// `limit`/`offset`. `None` when the result contains no owned, priced copy
    /// — a result worth nothing and a result worth an unknown amount are the
    /// same answer here, and neither is `$0.00`.
    ///
    /// It is computed here rather than by the client for two reasons, and the
    /// second is the durable one. A client holding one page can only sum that
    /// page, so under paging the figure had to be withheld unless the page WAS
    /// the result (pd-2g84) — the collection page holds the whole result today
    /// (pd-7z4o), but a field that is page-invariant by construction cannot
    /// lose that property the next time the endpoint is paged. And a sum the
    /// client computes is a *second* implementation of what a collection is
    /// worth: the number is a button that opens the value-over-time chart, and
    /// that chart's copies are `status = 'owned'` (`value_history`), so this
    /// one's are too. Owned mode also lists `ordered` copies — a card that is
    /// paid for and not here — and those are counted by `total`, drawn by the
    /// list, and deliberately not valued.
    ///
    /// Owned-only holds in catalog-wide mode as well ("All cards"), where the
    /// result is mostly printings nobody owns. Summing those would answer a
    /// different question — what completing the set would cost — which is a
    /// figure worth having and is not this one.
    pub total_value: Option<f64>,
    #[ts(type = "number")]
    pub limit: u32,
    #[ts(type = "number")]
    pub offset: u32,
}

/// Rows returned when a caller names no `limit`.
///
/// The default is bounded, not unbounded: catalog-wide mode matches one row per
/// printing in the catalog, so an unbounded default leaves a 44 MB response one
/// forgotten parameter away — which is exactly how it shipped. Truncation is
/// not silent, because [`SearchPage::total`] always describes the whole result.
pub const DEFAULT_LIMIT: u32 = 250;

/// The largest page a caller may ask for. Beyond this the request is refused,
/// not clamped — a caller that wants everything asks for [`Slice::All`] and
/// says so, rather than naming a number it hopes is big enough and being
/// quietly truncated when the catalog outgrows it.
pub const MAX_LIMIT: u32 = 1000;

#[derive(Clone, Copy, Default)]
enum Dir {
    #[default]
    Asc,
    Desc,
}

#[derive(Default)]
struct Ctx {
    has_status_filter: bool,
    catalog_wide: bool,
    order_by: Option<String>,
    order_dir: Dir,
}

/// A compiled query: the WHERE clause, its bound params, and the flags that
/// drive template selection and ordering.
pub struct CompiledSearch {
    where_sql: String,
    params: Vec<Value>,
    has_status_filter: bool,
    catalog_wide: bool,
    order_by: Option<String>,
    order_dir: Dir,
}

impl CompiledSearch {
    /// The compiled WHERE clause. A `1=0` sentinel anywhere in it marks a
    /// keyword/flag that parsed but could not be compiled — the corpus and
    /// generative tests assert it is absent for supported queries.
    pub fn where_sql(&self) -> &str {
        &self.where_sql
    }

    /// Whether every clause compiled to real SQL (no `1=0` sentinel).
    pub fn is_supported(&self) -> bool {
        !self.where_sql.contains("1=0")
    }

    /// Force catalog-wide mode (every printing, owned or not) even when no
    /// `is:missing`/`is:owned` flag is present — backs the "All cards" toggle,
    /// which shows owned and unowned printings together.
    pub fn set_catalog_wide(&mut self, on: bool) {
        if on {
            self.catalog_wide = true;
        }
    }

    /// Override the sort produced by `order:`/`direction:` modifiers with
    /// explicit values (e.g. from a column-header click in the UI).
    pub fn override_order(&mut self, sort: Option<&str>, dir: Option<&str>) {
        if let Some(s) = sort {
            self.order_by = Some(s.to_ascii_lowercase());
        }
        if let Some(d) = dir {
            self.order_dir = if d.eq_ignore_ascii_case("desc") {
                Dir::Desc
            } else {
                Dir::Asc
            };
        }
    }
}

/// The default search — every owned printing, no filter. Used for the empty
/// query (the collection page's initial view), where there is no AST to parse.
pub fn compile_all() -> CompiledSearch {
    CompiledSearch {
        where_sql: "1=1".to_string(),
        params: Vec::new(),
        has_status_filter: false,
        catalog_wide: false,
        order_by: None,
        order_dir: Dir::Asc,
    }
}

/// Compile a parsed query into SQL. `flags` is the `is:`-flag registry loaded
/// from the DB (`search_meta::load_flags`).
pub fn compile(ast: &Ast, flags: &[SearchFlag]) -> CompiledSearch {
    let mut ctx = Ctx::default();
    let (where_sql, params) = match extract_modifiers(ast, &mut ctx) {
        Some(stripped) => compile_node(&stripped, flags, &mut ctx),
        None => ("1=1".to_string(), Vec::new()),
    };
    CompiledSearch {
        where_sql,
        params,
        has_status_filter: ctx.has_status_filter,
        catalog_wide: ctx.catalog_wide,
        order_by: ctx.order_by,
        order_dir: ctx.order_dir,
    }
}

/// Execute a compiled query **unbounded** — every matching row.
///
/// This is the differential-oracle and test path. Anything serving an HTTP
/// response wants [`search_page`] instead: catalog-wide mode returns one row
/// per printing in the catalog (56k on the real one), which is how the
/// endpoint came to ship a 44 MB body.
pub fn search(conn: &Connection, compiled: &CompiledSearch) -> Result<Vec<SearchRow>> {
    run(conn, compiled, None)
}

/// How much of a result set a caller wants.
#[derive(Debug, Clone, Copy)]
pub enum Slice {
    /// `limit` rows starting at `offset` — the bounded default, and what any
    /// caller that draws a fixed number of rows should ask for.
    Page { limit: u32, offset: u32 },
    /// Every matching row, however many that is.
    ///
    /// What the collection page asks for (pd-7z4o): it holds the whole result
    /// as JS objects and renders only the slice under the viewport, so a page
    /// boundary would cut nothing a reader can see. That is affordable because
    /// the response is compressed (pd-2r0p) — the catalog-wide payload
    /// measures 44,334,174 B raw and 3,243,336 B gzipped, 13.7x, because 56k
    /// near-identical records repeat their keys — and because the query itself
    /// runs in about a second.
    ///
    /// There is no `offset` here on purpose. Skipping rows out of a result you
    /// asked for in full is a contradiction, so the endpoint refuses the pair
    /// rather than picking one of the two meanings.
    All,
}

/// Execute a compiled query as one [`Slice`], alongside the count of the whole
/// result set.
///
/// `total` is the count of the **unbounded** query, not of the rows returned,
/// so a client can render "N results" and page without a second request.
/// `limit` and `offset` are echoed back so a response is self-describing; for
/// [`Slice::All`] the echoed `limit` is the number of rows actually served,
/// which is what makes such a response describe itself as unpaged.
pub fn search_page(
    conn: &Connection,
    compiled: &CompiledSearch,
    slice: Slice,
) -> Result<SearchPage> {
    search_page_timed(conn, compiled, slice).map(|(page, _)| page)
}

/// Wall time of each stage of [`search_page`], in milliseconds, for the
/// `Server-Timing` header: `totals` (count + owned value), `rows` (the page
/// query) and `copies` (the per-printing copy expansion).
#[derive(Debug, Clone, Copy, Default)]
pub struct SearchTimings {
    pub totals_ms: f64,
    pub rows_ms: f64,
    pub copies_ms: f64,
}

/// [`search_page`] plus where its time went.
pub fn search_page_timed(
    conn: &Connection,
    compiled: &CompiledSearch,
    slice: Slice,
) -> Result<(SearchPage, SearchTimings)> {
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    let (total, total_value) = totals(conn, compiled)?;
    let mut timings = SearchTimings {
        totals_ms: ms(t),
        ..Default::default()
    };
    let page = match slice {
        Slice::Page { limit, offset } => Some((limit, offset)),
        Slice::All => None,
    };
    let (rows, rows_ms, copies_ms) = run_timed(conn, compiled, page)?;
    timings.rows_ms = rows_ms;
    timings.copies_ms = copies_ms;
    let (limit, offset) = match slice {
        Slice::Page { limit, offset } => (limit, offset),
        Slice::All => (u32::try_from(rows.len()).unwrap_or(u32::MAX), 0),
    };
    Ok((
        SearchPage {
            rows,
            total,
            total_value,
            limit,
            offset,
        },
        timings,
    ))
}

/// Everything the compiled query matches, ignoring any paging: how many rows,
/// and what the owned copies among them are worth
/// ([`SearchPage::total`] and [`SearchPage::total_value`]).
///
/// One statement, because the two are the same fact counted in different units
/// and a second statement would be a second execution of the same WHERE clause
/// — and, worse, of a WHERE clause that could drift from this one.
pub fn totals(conn: &Connection, compiled: &CompiledSearch) -> Result<(i64, Option<f64>)> {
    let sql = format!(
        "SELECT COUNT(*), SUM(({OWNED_VALUE_SUBQ})) {FROM_CLAUSE} WHERE {}",
        where_clause(compiled)
    );
    let row = conn.query_row(&sql, params_from_iter(compiled.params.iter()), |r| {
        Ok((r.get(0)?, r.get(1)?))
    })?;
    Ok(row)
}

/// Run the row query, optionally bounded to one `(limit, offset)` page.
fn run(
    conn: &Connection,
    compiled: &CompiledSearch,
    page: Option<(u32, u32)>,
) -> Result<Vec<SearchRow>> {
    run_timed(conn, compiled, page).map(|(rows, _, _)| rows)
}

/// [`run`], also returning the milliseconds spent on the row query and on the
/// copy expansion.
fn run_timed(
    conn: &Connection,
    compiled: &CompiledSearch,
    page: Option<(u32, u32)>,
) -> Result<(Vec<SearchRow>, f64, f64)> {
    let t = std::time::Instant::now();
    let sql = build_full_sql(compiled, page);
    // The paging bounds are bound parameters like every user value, appended
    // after the WHERE clause's own params in statement order.
    let mut params = compiled.params.clone();
    if let Some((limit, offset)) = page {
        params.push(Value::Integer(i64::from(limit)));
        params.push(Value::Integer(i64::from(offset)));
    }
    let mut stmt = conn.prepare(&sql)?;
    let mut rows: Vec<SearchRow> = stmt
        .query_map(params_from_iter(params.iter()), row_from)?
        .collect::<rusqlite::Result<_>>()?;
    let rows_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = std::time::Instant::now();
    if !rows.is_empty() {
        attach_copies(conn, &mut rows)?;
    }
    Ok((rows, rows_ms, t.elapsed().as_secs_f64() * 1000.0))
}

// ---------------------------------------------------------------------------
// Modifier extraction (order: / direction:)
// ---------------------------------------------------------------------------

fn extract_modifiers(ast: &Ast, ctx: &mut Ctx) -> Option<Ast> {
    match ast {
        Ast::Comparison { keyword, value, .. } if keyword == "order" => {
            ctx.order_by = Some(value.to_ascii_lowercase());
            None
        }
        Ast::Comparison { keyword, value, .. } if keyword == "direction" => {
            ctx.order_dir = if value.eq_ignore_ascii_case("desc") {
                Dir::Desc
            } else {
                Dir::Asc
            };
            None
        }
        Ast::And(children) => {
            let kept: Vec<Ast> = children
                .iter()
                .filter_map(|c| extract_modifiers(c, ctx))
                .collect();
            match kept.len() {
                0 => None,
                1 => Some(kept.into_iter().next().expect("len == 1")),
                _ => Some(Ast::And(kept)),
            }
        }
        other => Some(other.clone()),
    }
}

// ---------------------------------------------------------------------------
// Node compilation
// ---------------------------------------------------------------------------

fn compile_node(ast: &Ast, flags: &[SearchFlag], ctx: &mut Ctx) -> (String, Vec<Value>) {
    match ast {
        Ast::And(children) => combine(children, " AND ", flags, ctx),
        Ast::Or(children) => combine(children, " OR ", flags, ctx),
        Ast::Not(inner) => {
            let (sql, params) = compile_node(inner, flags, ctx);
            (format!("NOT ({sql})"), params)
        }
        Ast::NameSearch(term) => like_contains("cd.name", term),
        Ast::ExactName(name) => ("cd.name = ? COLLATE NOCASE".to_string(), vec![text(name)]),
        Ast::Comparison { keyword, op, value } => {
            compile_comparison(keyword, *op, value, flags, ctx)
        }
    }
}

fn combine(
    children: &[Ast],
    sep: &str,
    flags: &[SearchFlag],
    ctx: &mut Ctx,
) -> (String, Vec<Value>) {
    let mut parts = Vec::with_capacity(children.len());
    let mut params = Vec::new();
    for child in children {
        let (sql, p) = compile_node(child, flags, ctx);
        parts.push(format!("({sql})"));
        params.extend(p);
    }
    (parts.join(sep), params)
}

fn compile_comparison(
    keyword: &str,
    op: Op,
    value: &str,
    flags: &[SearchFlag],
    ctx: &mut Ctx,
) -> (String, Vec<Value>) {
    match keyword {
        // --- card identity / name ---
        "name" => match op {
            Op::Eq => ("cd.name = ? COLLATE NOCASE".to_string(), vec![text(value)]),
            Op::Ne => (
                "(cd.name IS NULL OR cd.name != ? COLLATE NOCASE)".to_string(),
                vec![text(value)],
            ),
            _ => like_contains("cd.name", value),
        },

        // --- card type / aspects / arenas / traits ---
        "card_type" => enum_eq("cd.card_type", op, value),
        "aspect" => json_array(op, "cd.aspects", value),
        "arena" => json_array(op, "cd.arenas", value),
        "trait" => json_array(op, "cd.traits", value),
        "keyword" => keyword_match(op, value),

        // --- numeric ---
        "cost" => numeric("cd.cost", op, value, Some("cd.cost IS NOT NULL")),
        "power" => numeric("cd.power", op, value, Some("cd.power IS NOT NULL")),
        "hp" => numeric("cd.hp", op, value, Some("cd.hp IS NOT NULL")),
        "year" => numeric(
            "CAST(SUBSTR(s.release_date, 1, 4) AS INTEGER)",
            op,
            value,
            None,
        ),

        // --- text contains ---
        "artist" => like_contains("p.artist", value),
        "oracle" => like_contains("cd.rules_text", value),
        "epic" => like_contains("cd.epic_action", value),

        // --- rarity (rank table for ordinals) ---
        "rarity" => rarity(op, value),

        // --- set (code / name) ---
        "set" => set(op, value),

        // --- legality: a card with no row is legal, as in deck checking ---
        "legality" => legality(value, false),
        "banned" => legality(value, true),

        // --- variant ---
        "variant" => {
            let frag = variant_match(value);
            if matches!(op, Op::Ne) {
                negate(frag)
            } else {
                frag
            }
        }
        "finish" => enum_eq("p.finish", op, value),

        // --- flags ---
        "is_flag" => is_flag(value, flags, ctx),
        "has_flag" => has_flag(value),

        // --- collection predicates (EXISTS over copies) ---
        "status" => {
            ctx.has_status_filter = true;
            let inner = if matches!(op, Op::Ne) {
                "c.status != ? COLLATE NOCASE"
            } else {
                "c.status = ? COLLATE NOCASE"
            };
            (exists_copy(inner), vec![text(value)])
        }
        "condition" => {
            let inner = if matches!(op, Op::Ne) {
                "c.condition != ? COLLATE NOCASE"
            } else {
                "c.condition = ? COLLATE NOCASE"
            };
            (exists_copy(inner), vec![text(condition_value(value))])
        }
        "grade" => {
            let (cond, p) = numeric(
                "c.grade_value",
                op,
                value,
                Some("c.grade_value IS NOT NULL"),
            );
            (exists_copy(&cond), p)
        }
        "grader" => (
            exists_copy("c.grade_company LIKE ? COLLATE NOCASE"),
            vec![text(pct(value))],
        ),
        "paid" => {
            let (cond, p) = numeric(
                "c.purchase_price",
                op,
                value,
                Some("c.purchase_price IS NOT NULL"),
            );
            (exists_copy(&cond), p)
        }
        "sale_price" => {
            let (cond, p) = numeric("c.sale_price", op, value, Some("c.sale_price IS NOT NULL"));
            (exists_copy(&cond), p)
        }
        "added" => {
            let cond = format!("SUBSTR(c.acquired_at, 1, 10) {} ?", num_op(op));
            (exists_copy(&cond), vec![text(value)])
        }
        "language" => (
            exists_copy("c.language LIKE ? COLLATE NOCASE"),
            vec![text(pct(value))],
        ),
        "source" => (
            exists_copy("c.source LIKE ? COLLATE NOCASE"),
            vec![text(pct(value))],
        ),
        "tag" => (
            exists_copy("c.tags LIKE ? COLLATE NOCASE"),
            vec![text(format!("%\"{value}\"%"))],
        ),
        "note" => (
            exists_copy("c.notes LIKE ? COLLATE NOCASE"),
            vec![text(pct(value))],
        ),
        "deck" => container(op, value, "deck_id", "decks"),
        "binder" => container(op, value, "binder_id", "binders"),

        // --- price (market) ---
        "price" => numeric(&format!("({MARKET_PRICE_EXPR})"), op, value, None),

        // --- quantity (per-printing, D4) ---
        "qty" => numeric(&format!("({OWNED_COUNT_SUBQ})"), op, value, None),

        // Parsed but not compilable.
        _ => ("1=0 /* unsupported keyword */".to_string(), Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// Per-kind helpers
// ---------------------------------------------------------------------------

fn num_op(op: Op) -> &'static str {
    match op {
        Op::Contains | Op::Eq => "=",
        Op::Ne => "!=",
        Op::Lt => "<",
        Op::Gt => ">",
        Op::Le => "<=",
        Op::Ge => ">=",
    }
}

fn text(s: impl Into<String>) -> Value {
    Value::Text(s.into())
}

fn pct(s: &str) -> String {
    format!("%{s}%")
}

fn negate((sql, params): (String, Vec<Value>)) -> (String, Vec<Value>) {
    (format!("NOT ({sql})"), params)
}

fn like_contains(col: &str, val: &str) -> (String, Vec<Value>) {
    (format!("{col} LIKE ? COLLATE NOCASE"), vec![text(pct(val))])
}

fn json_contains(col: &str, val: &str) -> (String, Vec<Value>) {
    (
        format!("{col} LIKE ? COLLATE NOCASE"),
        vec![text(format!("%\"{val}\"%"))],
    )
}

/// JSON-array membership with `:`/`=` = contains, `!=` = not-contains.
fn json_array(op: Op, col: &str, val: &str) -> (String, Vec<Value>) {
    let frag = json_contains(col, val);
    if matches!(op, Op::Ne) {
        negate(frag)
    } else {
        frag
    }
}

/// `keyword:raid` is any Raid; `keyword:"raid 2"` is Raid with that number.
/// `!=` negates either.
fn keyword_match(op: Op, val: &str) -> (String, Vec<Value>) {
    let val = val.trim();
    let (name, number) = match val.rsplit_once(' ') {
        Some((n, v)) if v.parse::<i64>().is_ok() => (n.trim(), Some(v.parse::<i64>().unwrap())),
        _ => (val, None),
    };
    let mut sql = "EXISTS (SELECT 1 FROM json_each(cd.keywords) \
                    WHERE json_extract(value, '$.name') = ? COLLATE NOCASE"
        .to_string();
    let mut params = vec![text(name)];
    if let Some(n) = number {
        sql.push_str(" AND json_extract(value, '$.value') = ?");
        params.push(Value::Integer(n));
    }
    sql.push(')');
    let frag = (sql, params);
    if matches!(op, Op::Ne) {
        negate(frag)
    } else {
        frag
    }
}

fn enum_eq(col: &str, op: Op, val: &str) -> (String, Vec<Value>) {
    match op {
        Op::Ne => (
            format!("({col} IS NULL OR {col} != ? COLLATE NOCASE)"),
            vec![text(val)],
        ),
        _ => (format!("{col} = ? COLLATE NOCASE"), vec![text(val)]),
    }
}

fn numeric(expr: &str, op: Op, val: &str, extra: Option<&str>) -> (String, Vec<Value>) {
    match val.parse::<f64>() {
        Ok(n) => {
            let cond = format!("{expr} {} ?", num_op(op));
            let full = match extra {
                Some(e) => format!("({e} AND {cond})"),
                None => format!("({cond})"),
            };
            (full, vec![Value::Real(n)])
        }
        Err(_) => ("1=0 /* invalid number */".to_string(), Vec::new()),
    }
}

fn rarity(op: Op, val: &str) -> (String, Vec<Value>) {
    match op {
        Op::Contains | Op::Eq => (
            "(p.rarity = ? COLLATE NOCASE OR p.rarity IN \
             (SELECT name FROM rarities WHERE grp = ? COLLATE NOCASE))"
                .to_string(),
            vec![text(val), text(val)],
        ),
        Op::Ne => (
            "NOT (p.rarity = ? COLLATE NOCASE OR p.rarity IN \
             (SELECT name FROM rarities WHERE grp = ? COLLATE NOCASE))"
                .to_string(),
            vec![text(val), text(val)],
        ),
        _ => (
            format!(
                "((SELECT rank FROM rarities WHERE name = p.rarity) {} \
                 (SELECT rank FROM rarities WHERE name = ? COLLATE NOCASE))",
                num_op(op)
            ),
            vec![text(val)],
        ),
    }
}

fn set(op: Op, val: &str) -> (String, Vec<Value>) {
    match op {
        Op::Eq => ("s.set_code = ? COLLATE NOCASE".to_string(), vec![text(val)]),
        Op::Ne => (
            "NOT (s.set_code = ? COLLATE NOCASE)".to_string(),
            vec![text(val)],
        ),
        _ => (
            "(s.set_code = ? COLLATE NOCASE OR s.name LIKE ? COLLATE NOCASE)".to_string(),
            vec![text(val), text(pct(val))],
        ),
    }
}

/// `legal:<format>` / `banned:<format>`. An unknown format matches nothing
/// rather than everything.
fn legality(format: &str, banned: bool) -> (String, Vec<Value>) {
    let known = "EXISTS (SELECT 1 FROM formats f WHERE f.format_id = ? COLLATE NOCASE)";
    let sql = if banned {
        format!(
            "({known} AND EXISTS (SELECT 1 FROM card_legality l \
               WHERE l.card_id = cd.card_id AND l.format_id = ? COLLATE NOCASE \
                 AND l.status = 'banned'))"
        )
    } else {
        format!(
            "({known} AND NOT EXISTS (SELECT 1 FROM card_legality l \
               WHERE l.card_id = cd.card_id AND l.format_id = ? COLLATE NOCASE \
                 AND l.status IN ('banned', 'not_legal')))"
        )
    };
    (sql, vec![text(format), text(format)])
}

fn variant_match(val: &str) -> (String, Vec<Value>) {
    let pat = pct(val);
    (
        "p.variant LIKE ? COLLATE NOCASE".to_string(),
        vec![text(&pat)],
    )
}

fn exists_copy(inner: &str) -> String {
    format!("EXISTS (SELECT 1 FROM collection c WHERE c.printing_id = p.printing_id AND ({inner}))")
}

fn condition_value(v: &str) -> String {
    match v.to_ascii_lowercase().as_str() {
        "nm" => "Near Mint",
        "lp" => "Lightly Played",
        "mp" => "Moderately Played",
        "hp" => "Heavily Played",
        "dmg" | "d" => "Damaged",
        _ => return v.to_string(),
    }
    .to_string()
}

fn container(op: Op, val: &str, fk: &str, table: &str) -> (String, Vec<Value>) {
    if val == "*" {
        let inner = if matches!(op, Op::Ne) {
            format!("c.{fk} IS NULL")
        } else {
            format!("c.{fk} IS NOT NULL")
        };
        return (exists_copy(&inner), Vec::new());
    }
    match op {
        Op::Eq => (
            format!(
                "EXISTS (SELECT 1 FROM collection c JOIN {table} x ON c.{fk} = x.id \
                 WHERE c.printing_id = p.printing_id AND x.name = ? COLLATE NOCASE)"
            ),
            vec![text(val)],
        ),
        Op::Ne => (
            format!(
                "EXISTS (SELECT 1 FROM collection c LEFT JOIN {table} x ON c.{fk} = x.id \
                 WHERE c.printing_id = p.printing_id AND (x.name IS NULL OR x.name != ? COLLATE NOCASE))"
            ),
            vec![text(val)],
        ),
        _ => (
            format!(
                "EXISTS (SELECT 1 FROM collection c JOIN {table} x ON c.{fk} = x.id \
                 WHERE c.printing_id = p.printing_id AND x.name LIKE ? COLLATE NOCASE)"
            ),
            vec![text(pct(val))],
        ),
    }
}

fn is_flag(value: &str, flags: &[SearchFlag], ctx: &mut Ctx) -> (String, Vec<Value>) {
    let lower = value.to_ascii_lowercase();
    let Some(flag) = flags.iter().find(|f| f.flag == lower) else {
        return ("1=0 /* unknown is: flag */".to_string(), Vec::new());
    };
    match flag.kind.as_str() {
        "computed" => match flag.predicate.as_deref() {
            Some("graded") => (exists_copy("c.graded = 1"), Vec::new()),
            Some("dupe") => (format!("({OWNED_COUNT_SUBQ}) >= 2"), Vec::new()),
            Some("unique") => ("cd.is_unique = 1".to_string(), Vec::new()),
            Some("owned") => {
                ctx.catalog_wide = true;
                (format!("({OWNED_COUNT_SUBQ}) > 0"), Vec::new())
            }
            Some("missing") => {
                ctx.catalog_wide = true;
                (format!("({OWNED_COUNT_SUBQ}) = 0"), Vec::new())
            }
            _ => ("1=0 /* unknown computed flag */".to_string(), Vec::new()),
        },
        "finish" => (
            "p.finish = ? COLLATE NOCASE".to_string(),
            vec![text(flag.match_str.clone().unwrap_or_default())],
        ),
        "variant_match" => {
            let m = flag.match_str.clone().unwrap_or_default();
            let pat = pct(&m);
            (
                "p.variant LIKE ? COLLATE NOCASE".to_string(),
                vec![text(&pat)],
            )
        }
        _ => ("1=0 /* unknown flag kind */".to_string(), Vec::new()),
    }
}

fn has_flag(value: &str) -> (String, Vec<Value>) {
    let sql = match value.to_ascii_lowercase().as_str() {
        "text" => "cd.rules_text IS NOT NULL AND cd.rules_text != ''",
        "epic" => "cd.epic_action IS NOT NULL AND cd.epic_action != ''",
        "keywords" => "cd.keywords != '[]'",
        "subtitle" => "cd.subtitle IS NOT NULL AND cd.subtitle != ''",
        "artist" => "p.artist IS NOT NULL AND p.artist != ''",
        _ => "1=0 /* unknown has: flag */",
    };
    (sql.to_string(), Vec::new())
}

// ---------------------------------------------------------------------------
// Full SQL assembly + row mapping
// ---------------------------------------------------------------------------

/// Every value `sort=` accepts, in the order the endpoint names them back when
/// it refuses one.
///
/// Every entry has an arm in `order_sql` below, and a test pins that: an entry
/// with no arm would fall through to the name fallback and quietly serve a
/// different order than the caller asked for.
///
/// **No key here orders across the `ATTACH` boundary from a tenant table.**
/// That is the property the list exists to hold: `value` and `adj` were
/// computed by a subquery joining the tenant's `collection` to the shared
/// catalog's prices across that boundary, and SQLite cannot index across
/// attached databases — so ordering by one could never be index-satisfied even
/// in principle (pd-tjym). They are
/// gone from this surface permanently, not provisionally: their successor is a
/// partitioned view whose result is small by construction, which can sort them
/// computed in flight. Both still RENDER — on the card modal, and the Adj.
/// column still draws — because rendering a computed value for the rows in a
/// window was never the cost.
///
/// What that is **not**, and pd-66hq measured rather than assumed: it is not
/// "every key is a stored scalar in one database", and none of these orderings
/// is index-satisfied. `qty` and `added` are still correlated subqueries over
/// the tenant's `collection`, `rarity` is a subquery, `set` is a joined
/// column, and `price` still COALESCEs the shared `prices` with tenant
/// `manual_prices` (pd-m4gw). Every one of them sorts in a temp b-tree, and no
/// index can change that — see `SORTS_THAT_STILL_SORT` in this file's tests
/// for the two structural reasons and pd-pvz0 for the shape that would fix it.
///
/// Nor is ordering what a large result's first paint costs: `order:value` and
/// `order:adj` cost what `order:name` costs (±10%), and dropping the ORDER BY
/// altogether saves 3%. The time is per-row projection, and removing `value`
/// and `adj` fixed an affordance and an unindexable ordering, not that number
/// (pd-66hq).
pub const SORT_KEYS: &[&str] = &[
    "name", "number", "set", "rarity", "type", "cost", "power", "hp", "aspects", "price", "qty",
    "added",
];

/// The canonical aspect order, from `docs/aspect-order.md`: the order in which a
/// set's collector numbering walks through the aspects of its non-leader,
/// non-base cards. A fixed list, never queried at sort time; the test
/// `aspect_order_constant_matches_the_set_numbering` re-derives it from a
/// fixture and fails if a set breaks the pattern.
pub const ASPECT_ORDER: [&str; 6] = [
    "Vigilance",
    "Command",
    "Aggression",
    "Cunning",
    "Villainy",
    "Heroism",
];

/// SQL for a card's sort key: the ranks in [`ASPECT_ORDER`] of its aspects in
/// the printed order, as two-digit groups closed by `99`, so keys compare
/// element by element and a longer list sorts before its own prefix
/// (`[Vigilance, Villainy]` before `[Vigilance]`, as the numbering has it). The
/// list is compared as printed, not as a set. NULL when the card has no aspects.
fn aspects_key_sql() -> String {
    let mut arms = String::new();
    for (i, a) in ASPECT_ORDER.iter().enumerate() {
        arms.push_str(&format!("WHEN '{a}' THEN '{:02}' ", i + 1));
    }
    format!(
        "(SELECT CASE WHEN COUNT(*) = 0 THEN NULL ELSE group_concat(r, '') || '99' END \
         FROM (SELECT CASE j.value {arms}ELSE '98' END AS r \
               FROM json_each(cd.aspects) j ORDER BY j.key))"
    )
}

/// Whether `sort=<key>` names a column this query can order by.
///
/// Case-insensitive, because [`CompiledSearch::override_order`] lowercases what
/// it stores and a caller validating before that point must agree with it.
pub fn is_sortable(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    SORT_KEYS.contains(&key.as_str())
}

fn order_sql(order_by: Option<&str>) -> String {
    match order_by.unwrap_or("name") {
        "number" => NUMBER_KEY.to_string(),
        // release_date is only the primary key here; build_full_sql adds
        // set_code as the secondary set-level key so sets sharing a date stay
        // grouped.
        "set" => "s.release_date".to_string(),
        "rarity" => "(SELECT sort_value FROM rarity_marks WHERE name = p.rarity)".to_string(),
        "type" => "cd.card_type".to_string(),
        "cost" => "cd.cost".to_string(),
        "power" => "cd.power".to_string(),
        "hp" => "cd.hp".to_string(),
        // The real ordering is assembled in `build_full_sql`; this arm only
        // keeps the key off the name fallback.
        "aspects" => "cd.aspects".to_string(),
        "price" => format!("({MARKET_PRICE_EXPR})"),
        "added" => {
            "(SELECT MAX(c.acquired_at) FROM collection c WHERE c.printing_id = p.printing_id)"
                .to_string()
        }
        "qty" => format!("({OWNED_COUNT_SUBQ})"),
        "name" => "cd.name".to_string(),
        // Anything else orders by name. `sort=` never reaches here — the
        // endpoint refuses a key outside `SORT_KEYS` — but the query language's
        // own `order:` modifier takes a free-text value, and an unrecognised
        // one has always fallen back rather than failed the whole query.
        _ => "cd.name".to_string(),
    }
}

/// Owned mode requires an owned/ordered copy unless the query already
/// constrains status explicitly; catalog-wide mode keeps the catalog.
fn where_clause(c: &CompiledSearch) -> String {
    if c.catalog_wide || c.has_status_filter {
        c.where_sql.clone()
    } else {
        format!(
            "EXISTS (SELECT 1 FROM collection c WHERE c.printing_id = p.printing_id \
             AND c.status IN ('owned', 'ordered')) AND ({})",
            c.where_sql
        )
    }
}

fn build_full_sql(c: &CompiledSearch, page: Option<(u32, u32)>) -> String {
    let where_clause = where_clause(c);
    let dir = if matches!(c.order_dir, Dir::Desc) {
        "DESC"
    } else {
        "ASC"
    };
    // `p.printing_id` is unique, so the ORDER BY is a total order. Without a
    // unique final key, rows tied on the sort column (a card's Standard and
    // Hyperspace printings share a name) may sit in either order from one
    // statement to the next, and OFFSET paging then drops and duplicates them.
    let paging = if page.is_some() {
        " LIMIT ? OFFSET ?"
    } else {
        ""
    };
    // sort=set needs two set-level keys under dir so that sets sharing a
    // release_date still group correctly; within a set, collector number is
    // always ascending regardless of dir.
    let order_clause = if c.order_by.as_deref() == Some("set") {
        format!("s.release_date {dir}, s.set_code {dir}, {NUMBER_KEY} ASC, p.printing_id ASC")
    } else if c.order_by.as_deref() == Some("aspects") {
        // Unaspected cards last in both directions; cost then name break ties
        // ascending, like the number tiebreak.
        let key = aspects_key_sql();
        format!(
            "({key} IS NULL) ASC, {key} {dir}, cd.cost ASC, cd.name ASC, {NUMBER_KEY} ASC, \
             p.printing_id ASC"
        )
    } else {
        let order_col = order_sql(c.order_by.as_deref());
        format!("{order_col} {dir}, {NUMBER_KEY} ASC, p.printing_id ASC")
    };
    format!(
        "SELECT p.printing_id, cd.card_id, p.set_code, s.name AS set_name, p.number, \
                cd.name, cd.subtitle, p.rarity, cd.card_type, cd.cost, cd.power, cd.hp, \
                cd.aspects, cd.arenas, \
                {MARKET_PRICE_EXPR} AS market_price, \
                p.image_url, p.variant, p.finish, \
                ({OWNED_COUNT_SUBQ}) AS owned_count, cd.traits, p.back_image_url \
         {FROM_CLAUSE} \
         WHERE {where_clause} \
         ORDER BY {order_clause}{paging}"
    )
}

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<SearchRow> {
    let owned_count: i64 = r.get(18)?;
    Ok(SearchRow {
        printing_id: r.get(0)?,
        card_id: r.get(1)?,
        set_code: r.get(2)?,
        set_name: r.get(3)?,
        number: r.get(4)?,
        name: r.get(5)?,
        subtitle: r.get(6)?,
        rarity: r.get(7)?,
        card_type: r.get(8)?,
        cost: r.get(9)?,
        power: r.get(10)?,
        hp: r.get(11)?,
        aspects: r.get(12)?,
        arenas: r.get(13)?,
        traits: r.get(19)?,
        market_price: r.get(14)?,
        image_url: r.get(15)?,
        back_image_url: r.get(20)?,
        variant: r.get(16)?,
        finish: r.get(17)?,
        owned: owned_count > 0,
        owned_count,
        copies: Vec::new(),
    })
}

fn attach_copies(conn: &Connection, rows: &mut [SearchRow]) -> Result<()> {
    // Chunk the IN-clause so a catalog-wide result (is:missing returns one row
    // per unowned printing — tens of thousands on the real catalog) never
    // exceeds SQLite's SQLITE_MAX_VARIABLE_NUMBER. 900 stays under even the
    // legacy 999 limit.
    const CHUNK: usize = 900;
    let ids: Vec<String> = rows.iter().map(|r| r.printing_id.clone()).collect();
    let mut by_printing: HashMap<String, Vec<CopySummary>> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT printing_id, id, condition, language, status, graded, purchase_price, \
                    acquired_at, binder_id, deck_id \
             FROM collection WHERE printing_id IN ({placeholders}) ORDER BY id"
        );
        let mut stmt = conn.prepare(&sql)?;
        let mapped = stmt.query_map(params_from_iter(chunk.iter()), |r| {
            let printing_id: String = r.get(0)?;
            Ok((
                printing_id,
                CopySummary {
                    id: r.get(1)?,
                    condition: r.get(2)?,
                    language: r.get(3)?,
                    status: r.get(4)?,
                    graded: r.get(5)?,
                    purchase_price: r.get(6)?,
                    acquired_at: r.get(7)?,
                    binder_id: r.get(8)?,
                    deck_id: r.get(9)?,
                },
            ))
        })?;
        for entry in mapped {
            let (printing_id, copy) = entry?;
            by_printing.entry(printing_id).or_default().push(copy);
        }
    }
    for row in rows.iter_mut() {
        if let Some(copies) = by_printing.remove(&row.printing_id) {
            row.copies = copies;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{connect_user, open_shared, search_meta, testkit};
    use dumpstar_core::query::{KeywordRegistry, parse};

    struct Fix {
        _dir: tempfile::TempDir,
        shared: std::path::PathBuf,
        conn: Connection,
        registry: KeywordRegistry,
        flags: Vec<SearchFlag>,
    }

    fn fixture() -> Fix {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            testkit::set_on(&c, "SOR", "Spark of Rebellion", "2024-03-08");
            testkit::set_on(&c, "SHD", "Shadows of the Galaxy", "2024-07-12");
            // Luke (Vigilance/Heroism, power 3, hp 5), Millennium Falcon
            // (Cunning/Heroism, Space, power 3, hp 3), Han Solo (Aggression/
            // Villainy, power 7, hp 6) and Vanquish, an Event with no stats.
            testkit::card_full(
                &c,
                "luke",
                "Luke Skywalker",
                Some("Faithful Friend"),
                "Unit",
                Some(3),
                Some(3),
                Some(5),
                r#"["Vigilance","Heroism"]"#,
                r#"["Force","Rebel","Jedi"]"#,
                r#"["Ground"]"#,
                Some("On Attack: Heal 2 damage from a base."),
            );
            testkit::card_full(
                &c,
                "falcon",
                "Millennium Falcon",
                Some("Piece of Junk"),
                "Unit",
                Some(2),
                Some(3),
                Some(3),
                r#"["Cunning","Heroism"]"#,
                r#"["Vehicle","Transport","Underworld"]"#,
                r#"["Space"]"#,
                Some("Piloting [2]."),
            );
            testkit::card_full(
                &c,
                "han",
                "Han Solo",
                Some("Reluctant Hero"),
                "Unit",
                Some(6),
                Some(7),
                Some(6),
                r#"["Aggression","Villainy"]"#,
                r#"["Underworld","Rebel"]"#,
                r#"["Ground"]"#,
                Some("When Played: Deal 2 damage to a unit."),
            );
            // An Event: no stats, no arena. Unowned, so it only surfaces
            // catalog-wide and leaves the owned-mode tests alone.
            testkit::card_full(
                &c,
                "vanquish",
                "Vanquish",
                None,
                "Event",
                Some(5),
                None,
                None,
                r#"["Vigilance"]"#,
                "[]",
                "[]",
                Some("Defeat a non-leader unit."),
            );
            testkit::printing(&c, "luke-foil", "luke", "SOR", "005", "Standard", "Foil");
            testkit::printing(
                &c,
                "falcon-std",
                "falcon",
                "SHD",
                "252",
                "Standard",
                "Normal",
            );
            testkit::printing(
                &c,
                "falcon-hyper",
                "falcon",
                "SHD",
                "512",
                "Hyperspace",
                "Normal",
            );
            testkit::printing(&c, "han-std", "han", "SOR", "010", "Standard", "Normal");
            testkit::printing(
                &c,
                "vanquish-std",
                "vanquish",
                "SOR",
                "193",
                "Standard",
                "Normal",
            );
            c.execute_batch(
                "UPDATE printings SET rarity = 'Legendary', artist = 'Borja Pindado' \
                    WHERE printing_id = 'luke-foil';
                 UPDATE printings SET rarity = 'Common', artist = 'Ralph McQuarrie' \
                    WHERE printing_id = 'falcon-std';
                 UPDATE printings SET rarity = 'Common', artist = 'Ralph McQuarrie' \
                    WHERE printing_id = 'falcon-hyper';
                 UPDATE printings SET rarity = 'Rare', artist = 'Ryan Grayson' \
                    WHERE printing_id = 'han-std';
                 UPDATE printings SET rarity = 'Uncommon' WHERE printing_id = 'vanquish-std';",
            )
            .unwrap();
        }
        // `connect_user` seeds the real multipliers from data/conditions.json
        // into the collection — `order:value` is defined in terms of them, so
        // a fixture without them could not tell a condition-adjusted sum from
        // a raw one.
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        // Own two Lukes (a dupe) and one Falcon; Han and Vanquish are unowned.
        conn.execute(
            "INSERT INTO collection (printing_id,condition,language,acquired_at,source,status)
             VALUES
             ('luke-foil','Near Mint','English','2024-01-01','manual_id','owned'),
             ('luke-foil','Lightly Played','English','2024-02-01','manual_id','owned'),
             ('falcon-std','Near Mint','English','2025-03-01','manual_id','owned')",
            [],
        )
        .unwrap();
        let registry = search_meta::load_registry(&conn).unwrap();
        let flags = search_meta::load_flags(&conn).unwrap();
        Fix {
            _dir: dir,
            shared,
            conn,
            registry,
            flags,
        }
    }

    impl Fix {
        /// Price a catalog printing the way the catalog does: a market quote
        /// in `shared.prices`. Reopens the catalog read-write, because the
        /// search connection has it attached read-only — which is the
        /// property under test.
        fn set_catalog_price(&self, printing_id: &str, price: f64) {
            let c = open_shared(&self.shared).unwrap();
            testkit::price(&c, printing_id, "2025-01-01", price);
        }

        fn compile(&self, q: &str) -> CompiledSearch {
            let ast = parse(q, &self.registry).unwrap();
            compile(&ast, &self.flags)
        }
        fn ids(&self, q: &str) -> Vec<String> {
            let c = self.compile(q);
            let mut ids: Vec<String> = search(&self.conn, &c)
                .unwrap()
                .into_iter()
                .map(|r| r.printing_id)
                .collect();
            ids.sort();
            ids
        }
    }

    // --- compile-shape assertions ------------------------------------------

    #[test]
    fn compiles_card_predicates_to_sql() {
        let f = fixture();
        assert!(
            f.compile("a:vigilance")
                .where_sql
                .contains("cd.aspects LIKE")
        );
        assert!(
            f.compile("arena:space")
                .where_sql
                .contains("cd.arenas LIKE")
        );
        assert!(
            f.compile("trait:rebel")
                .where_sql
                .contains("cd.traits LIKE")
        );
        assert!(f.compile("hp>=5").where_sql.contains("cd.hp"));
        assert!(f.compile("cost<=3").where_sql.contains("cd.cost"));
        assert!(f.compile("s:shd").where_sql.contains("s.set_code"));
        assert!(f.compile("rarity>=rare").where_sql.contains("rarities"));
        assert!(
            f.compile("legal:premier")
                .where_sql
                .contains("card_legality")
        );
    }

    #[test]
    fn status_sets_flag_and_is_missing_sets_catalog_wide() {
        let f = fixture();
        let s = f.compile("status:sold");
        assert!(s.has_status_filter);
        assert!(s.where_sql.contains("c.status"));
        let m = f.compile("is:missing");
        assert!(m.catalog_wide);
        assert!(m.where_sql.contains("= 0"));
    }

    #[test]
    fn modifiers_extracted_out_of_where() {
        let f = fixture();
        let c = f.compile("order:price direction:desc");
        assert_eq!(c.where_sql, "1=1");
    }

    #[test]
    fn invalid_and_unknown_emit_sentinel() {
        let f = fixture();
        assert!(f.compile("hp>=abc").where_sql.contains("1=0"));
        assert!(f.compile("is:nonsenseflag").where_sql.contains("1=0"));
    }

    // --- execution against the fixture -------------------------------------

    #[test]
    fn owned_mode_only_returns_owned_printings() {
        let f = fixture();
        // Vigilance cards I own → Luke; Vanquish (Vigilance, unowned) excluded.
        assert_eq!(f.ids("a:vigilance"), vec!["luke-foil"]);
        // Default owned mode never surfaces the unowned Han Solo.
        assert!(f.ids("a:aggression").is_empty());
    }

    #[test]
    fn is_missing_surfaces_unowned() {
        let f = fixture();
        let c = f.compile("is:missing a:aggression");
        let rows = search(&f.conn, &c).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].printing_id, "han-std");
        assert!(!rows[0].owned);
        assert_eq!(rows[0].owned_count, 0);
        assert!(rows[0].copies.is_empty());
    }

    // Regression for dumpstar-2gd: attach_copies built one bound parameter
    // per result row, so a catalog-wide result (is:missing returns one row per
    // unowned printing — tens of thousands on the real catalog) blew past
    // SQLite's SQLITE_MAX_VARIABLE_NUMBER -> "too many SQL variables" -> 500.
    // The UI fixture is too small to trip the limit. Drive attach_copies past
    // it directly here; one of the synthetic rows matches an owned copy so we
    // also prove copies still attach correctly across chunk boundaries.
    #[test]
    fn attach_copies_handles_more_rows_than_sqlite_variable_limit() {
        let f = fixture();
        let blank = |pid: &str| SearchRow {
            printing_id: pid.to_string(),
            card_id: String::new(),
            set_code: String::new(),
            set_name: String::new(),
            number: String::new(),
            name: String::new(),
            subtitle: None,
            rarity: None,
            card_type: None,
            cost: None,
            power: None,
            hp: None,
            aspects: None,
            arenas: None,
            traits: None,
            market_price: None,
            image_url: None,
            back_image_url: None,
            variant: String::new(),
            finish: String::new(),
            owned: false,
            owned_count: 0,
            copies: Vec::new(),
        };
        // 40_000 > the 32_766 default limit, and well past the legacy 999.
        let mut rows: Vec<SearchRow> = (0..40_000).map(|i| blank(&format!("p{i}"))).collect();
        // A real owned printing dropped in the middle, far past chunk 1.
        rows[20_000] = blank("luke-foil");

        attach_copies(&f.conn, &mut rows).unwrap();

        assert_eq!(
            rows[20_000].copies.len(),
            2,
            "owned copies attach across chunks"
        );
        assert!(rows[0].copies.is_empty());
    }

    // Full-path companion to attach_copies_handles_more_rows_than_sqlite_variable_limit
    // and the headline backend guard for dumpstar-2o1. The committed UI
    // fixture has a handful of printings, so neither the IN-clause cliff nor any
    // other per-row variable build is exercised at fixture scale — both
    // All-cards bugs shipped to prod invisibly. Build a catalog with more
    // unowned printings than SQLite's default SQLITE_MAX_VARIABLE_NUMBER
    // (32766) and run the real `is:missing` query end to end (build_full_sql +
    // attach_copies). Regresses loudly with "too many SQL variables" -> 500 if
    // any stage stops chunking.
    #[test]
    fn is_missing_survives_prod_scale_catalog() {
        const N: usize = 35_000; // > 32_766 default SQLITE_MAX_VARIABLE_NUMBER
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            testkit::set_on(&c, "BIG", "Big Set", "2024-01-01");
            let tx = c.transaction().unwrap();
            {
                let mut card_stmt = tx
                    .prepare("INSERT INTO cards (card_id,name,card_type) VALUES (?1,?2,'Unit')")
                    .unwrap();
                let mut pr_stmt = tx
                    .prepare(
                        "INSERT INTO printings (printing_id,card_id,set_code,number,variant,finish)
                         VALUES (?1,?2,'BIG',?3,'Standard','Normal')",
                    )
                    .unwrap();
                for i in 0..N {
                    let card_id = format!("big-{i}");
                    card_stmt
                        .execute(rusqlite::params![card_id, format!("Trooper {i}")])
                        .unwrap();
                    pr_stmt
                        .execute(rusqlite::params![
                            format!("{card_id}-normal"),
                            card_id,
                            i.to_string()
                        ])
                        .unwrap();
                }
            }
            tx.commit().unwrap();
        }
        // No collection rows at all → every printing is missing.
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let registry = search_meta::load_registry(&conn).unwrap();
        let flags = search_meta::load_flags(&conn).unwrap();
        let compiled = compile(&parse("is:missing", &registry).unwrap(), &flags);
        assert!(compiled.catalog_wide);

        let rows = search(&conn, &compiled).unwrap();
        assert_eq!(
            rows.len(),
            N,
            "every unowned printing surfaces — no 'too many SQL variables' 500"
        );
        assert!(rows.iter().all(|r| !r.owned));
    }

    #[test]
    fn owned_row_carries_copies_and_count() {
        let f = fixture();
        let c = f.compile("luke");
        let rows = search(&f.conn, &c).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].owned);
        assert_eq!(rows[0].owned_count, 2);
        assert_eq!(rows[0].copies.len(), 2);
    }

    // --- payload shape (pd-lk8v) -------------------------------------------

    // Every field here is paid once per row — for its value *and* for its key
    // name. Back when catalog-wide mode returned the whole catalog in one body
    // (~57k rows on prod) one over-long field was 54% of a 44 MB response. The
    // endpoint pages now (pd-jsby), which caps the multiplier without changing
    // the argument.
    //
    // Named explicitly, not size-asserted: a byte budget rots, a key set does
    // not. Adding a field to SearchRow fails this test, which is the point —
    // the addition should be a decision, not a diff nobody measured.
    #[test]
    fn list_payload_carries_only_what_the_list_renders() {
        let f = fixture();
        let rows = search(&f.conn, &f.compile("luke")).unwrap();
        let json = serde_json::to_value(&rows[0]).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .expect("SearchRow serializes to an object")
            .keys()
            .map(String::as_str)
            .collect();
        // serde_json orders object keys alphabetically, so this list is too.
        assert_eq!(
            keys,
            vec![
                "arenas",
                "aspects",
                "back_image_url",
                "card_id",
                "card_type",
                "copies",
                "cost",
                "finish",
                "hp",
                "image_url",
                "market_price",
                "name",
                "number",
                "owned",
                "owned_count",
                "power",
                "printing_id",
                "rarity",
                "set_code",
                "set_name",
                "subtitle",
                "traits",
                "variant",
            ],
            "a field the list does not render belongs on CardDetail, not here"
        );
        // What the card modal owns, spelled out so a revert reads as one.
        for gone in ["rules_text", "artist", "variant_description"] {
            assert!(
                !keys.contains(&gone),
                "{gone} is card-modal data — it must not ride the list payload"
            );
        }
    }

    // Aspects and arenas ride the list as the raw JSON arrays the catalog
    // stores; the client parses them. A card with none (an Event has no arena)
    // carries the empty array, not NULL.
    #[test]
    fn aspects_and_arenas_ride_as_json_arrays() {
        let f = fixture();
        let rows = search(&f.conn, &f.compile("luke")).unwrap();
        assert_eq!(
            rows[0].aspects.as_deref(),
            Some(r#"["Vigilance","Heroism"]"#)
        );
        assert_eq!(rows[0].arenas.as_deref(), Some(r#"["Ground"]"#));
        assert_eq!(rows[0].card_type.as_deref(), Some("Unit"));
        assert_eq!(rows[0].subtitle.as_deref(), Some("Faithful Friend"));
        assert_eq!(
            (rows[0].cost, rows[0].power, rows[0].hp),
            (Some(3), Some(3), Some(5))
        );

        let event = search(&f.conn, &f.compile("is:missing vanquish")).unwrap();
        assert_eq!(event.len(), 1);
        assert_eq!(event[0].printing_id, "vanquish-std");
        assert_eq!(event[0].arenas.as_deref(), Some("[]"));
        assert_eq!(event[0].subtitle, None);
        assert_eq!((event[0].power, event[0].hp), (None, None));
    }

    #[test]
    fn qty_counts_per_printing() {
        let f = fixture();
        // Luke's printing has 2 copies; the Falcon has 1.
        assert_eq!(f.ids("qty>=2"), vec!["luke-foil"]);
        assert!(f.ids("falcon qty>=2").is_empty());
        assert_eq!(f.ids("is:dupe"), vec!["luke-foil"]);
    }

    #[test]
    fn keyword_unique_and_epic_terms() {
        let f = fixture();
        {
            let c = open_shared(&f.shared).unwrap();
            c.execute_batch(
                r#"UPDATE cards SET keywords='[{"name":"Raid","value":2},{"name":"Sentinel","value":null}]',
                                    is_unique=1 WHERE card_id='luke';
                   UPDATE cards SET keywords='[{"name":"Piloting","value":null}]' WHERE card_id='falcon';
                   UPDATE cards SET epic_action='Deal 3 damage to a unit.' WHERE card_id='han';"#,
            )
            .unwrap();
        }
        assert_eq!(f.ids("k:sentinel"), vec!["luke-foil"]);
        assert_eq!(f.ids("keyword:raid"), vec!["luke-foil"]);
        assert_eq!(f.ids("k:\"raid 2\""), vec!["luke-foil"]);
        assert!(f.ids("k:\"raid 3\"").is_empty());
        assert_eq!(f.ids("k:piloting"), vec!["falcon-std"]);
        assert!(!f.ids("k!=sentinel").contains(&"luke-foil".to_string()));
        assert_eq!(f.ids("is:unique"), vec!["luke-foil"]);
        assert_eq!(f.ids("is:missing epic:damage"), vec!["han-std"]);
        assert_eq!(f.ids("is:missing has:epic"), vec!["han-std"]);
    }

    #[test]
    fn numeric_and_rarity_ordinals() {
        let f = fixture();
        // hp>=5 among owned → Luke (5); the Falcon (3) excluded.
        assert_eq!(f.ids("hp>=5"), vec!["luke-foil"]);
        // rarity>=rare → Legendary (Luke); Common (Falcon) excluded.
        assert_eq!(f.ids("rarity>=rare"), vec!["luke-foil"]);
    }

    #[test]
    fn finish_and_variant_flags() {
        let f = fixture();
        assert_eq!(f.ids("is:foil"), vec!["luke-foil"]);
        assert_eq!(f.ids("is:nonfoil"), vec!["falcon-std"]);
        assert_eq!(f.ids("f:foil"), vec!["luke-foil"]);
        assert_eq!(f.ids("is:standard"), vec!["falcon-std", "luke-foil"]);
        // Owned mode: the Hyperspace Falcon is not owned.
        assert!(f.ids("is:hyperspace").is_empty());
        assert_eq!(
            f.ids("is:missing is:hyperspace"),
            vec!["falcon-hyper"],
            "catalog-wide surfaces the unowned Hyperspace printing"
        );
        assert_eq!(f.ids("v:standard c:3"), vec!["luke-foil"]);
    }

    #[test]
    fn set_by_code_and_name() {
        let f = fixture();
        // s:shd → Shadows of the Galaxy; only the owned Falcon printing.
        assert_eq!(f.ids("s:shd"), vec!["falcon-std"]);
        assert_eq!(f.ids("s:shadows"), vec!["falcon-std"]);
        assert_eq!(f.ids("s:sor"), vec!["luke-foil"]);
    }

    #[test]
    fn swu_vocabulary_compiles_and_filters() {
        let f = fixture();
        assert_eq!(f.ids("arena:space"), vec!["falcon-std"]);
        assert_eq!(f.ids("trait:jedi"), vec!["luke-foil"]);
        assert_eq!(f.ids("t:unit cost<=2"), vec!["falcon-std"]);
        assert_eq!(f.ids("p>=3"), vec!["falcon-std", "luke-foil"]);
        assert_eq!(f.ids("o:heal"), vec!["luke-foil"]);
        assert_eq!(f.ids("has:subtitle"), vec!["falcon-std", "luke-foil"]);
        assert_eq!(f.ids("artist:mcquarrie"), vec!["falcon-std"]);
        assert_eq!(f.ids("year:2024"), vec!["falcon-std", "luke-foil"]);
        assert_eq!(f.ids("a:heroism"), vec!["falcon-std", "luke-foil"]);
    }

    #[test]
    fn negation_and_boolean() {
        let f = fixture();
        // Owned, not Vigilance → the Falcon only.
        assert_eq!(f.ids("-a:vigilance"), vec!["falcon-std"]);
        // OR across owned printings.
        assert_eq!(
            f.ids("a:vigilance or a:cunning"),
            vec!["falcon-std", "luke-foil"]
        );
    }

    // --- paging (pd-jsby) ---------------------------------------------------

    /// Catalog-wide, empty query: every fixture printing, the shape the
    /// unbounded endpoint used to return 56k rows of.
    fn all_printings() -> CompiledSearch {
        let mut c = compile_all();
        c.set_catalog_wide(true);
        c
    }

    /// The printings a sort returns, in order, catalog-wide.
    fn ordered(f: &Fix, sort: &str, dir: &str) -> Vec<String> {
        let mut c = all_printings();
        c.override_order(Some(sort), Some(dir));
        search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.printing_id)
            .collect()
    }

    // --- the sort keys the collection table hands to the server ------------
    //
    // Paging moved that page's sort off the client (pd-tsqd): sorting 250 of
    // 56,635 rows in the browser orders the wrong 250. Every column it offers
    // therefore has to be expressible here.

    #[test]
    fn order_by_type_sorts_on_card_type() {
        let f = fixture();
        // Three Units and one Event (Vanquish, vanquish-std).
        let asc = ordered(&f, "type", "asc");
        assert_eq!(
            asc.first().map(String::as_str),
            Some("vanquish-std"),
            "Event before Unit: {asc:?}"
        );
        let desc = ordered(&f, "type", "desc");
        assert_eq!(desc.last().map(String::as_str), Some("vanquish-std"));
    }

    #[test]
    fn order_by_cost_sorts_on_resource_cost() {
        let f = fixture();
        // Falcon 2 (two printings, tied: number breaks it), Luke 3,
        // Vanquish 5, Han 6.
        assert_eq!(
            ordered(&f, "cost", "asc"),
            vec![
                "falcon-std",
                "falcon-hyper",
                "luke-foil",
                "vanquish-std",
                "han-std",
            ]
        );
    }

    #[test]
    fn order_by_power_puts_the_unpowered_first_ascending() {
        let f = fixture();
        // The Event has no power and NULL sorts first ascending; Luke and the
        // Falcon tie on 3 and fall to collector number; Han is 7.
        assert_eq!(
            ordered(&f, "power", "asc"),
            vec![
                "vanquish-std",
                "luke-foil",
                "falcon-std",
                "falcon-hyper",
                "han-std",
            ]
        );
    }

    // --- sort=aspects ------------------------------------------------------

    /// Two sets numbered the way the real ones are (docs/aspect-order.md): per
    /// primary aspect the dual-aspect cards run first, then the single-aspect
    /// ones; Villainy and Heroism alone near the end, unaspected cards last. The
    /// second set skips Command and prints one list in reverse. A Leader numbered
    /// apart (000) is not evidence for the order.
    fn aspect_fixture() -> Fix {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            testkit::set_on(&c, "AS1", "Aspect One", "2024-01-01");
            testkit::set_on(&c, "AS2", "Aspect Two", "2024-06-01");
            // (set, number, id, type, cost, aspects)
            let cards: &[(&str, &str, &str, &str, i64, &str)] = &[
                ("AS1", "000", "ldr", "Leader", 6, r#"["Heroism"]"#),
                ("AS1", "001", "vv", "Unit", 2, r#"["Vigilance","Villainy"]"#),
                ("AS1", "002", "vh", "Event", 1, r#"["Vigilance","Heroism"]"#),
                ("AS1", "003", "v5", "Unit", 5, r#"["Vigilance"]"#),
                ("AS1", "004", "v2", "Upgrade", 2, r#"["Vigilance"]"#),
                ("AS1", "005", "ch", "Unit", 3, r#"["Command","Heroism"]"#),
                ("AS1", "006", "c3", "Unit", 3, r#"["Command"]"#),
                ("AS1", "007", "a4", "Unit", 4, r#"["Aggression"]"#),
                ("AS1", "008", "u3", "Unit", 3, r#"["Cunning"]"#),
                ("AS1", "009", "vl", "Unit", 2, r#"["Villainy"]"#),
                ("AS1", "010", "h1", "Event", 1, r#"["Heroism"]"#),
                ("AS1", "011", "none", "Unit", 0, "[]"),
                ("AS2", "001", "v7", "Unit", 7, r#"["Vigilance"]"#),
                ("AS2", "002", "ah", "Unit", 6, r#"["Aggression","Heroism"]"#),
                ("AS2", "003", "a6", "Unit", 6, r#"["Aggression"]"#),
                ("AS2", "004", "u6", "Unit", 6, r#"["Cunning"]"#),
                ("AS2", "005", "hv", "Unit", 6, r#"["Heroism","Vigilance"]"#),
            ];
            for (set, number, id, ty, cost, aspects) in cards {
                testkit::card_full(
                    &c,
                    id,
                    id,
                    None,
                    ty,
                    Some(*cost),
                    None,
                    None,
                    aspects,
                    "[]",
                    "[]",
                    None,
                );
                testkit::printing(
                    &c,
                    &format!("{id}-p"),
                    id,
                    set,
                    number,
                    "Standard",
                    "Normal",
                );
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let registry = search_meta::load_registry(&conn).unwrap();
        let flags = search_meta::load_flags(&conn).unwrap();
        Fix {
            _dir: dir,
            shared,
            conn,
            registry,
            flags,
        }
    }

    /// Each set's aspect order as its numbering gives it: walk the set's
    /// non-leader, non-base cards by collector number and note where each
    /// aspect first stands alone on a card.
    fn derived_set_orders(conn: &Connection) -> Vec<Vec<String>> {
        let mut stmt = conn
            .prepare(
                "SELECT p.set_code, cd.aspects \
                 FROM printings p JOIN cards cd ON cd.card_id = p.card_id \
                 WHERE cd.card_type NOT IN ('Leader', 'Base') \
                   AND p.variant = 'Standard' AND p.finish = 'Normal' \
                 ORDER BY p.set_code, CAST(p.number AS INTEGER)",
            )
            .unwrap();
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut sets: Vec<(String, Vec<String>)> = Vec::new();
        for (set, aspects) in rows {
            if sets.last().map(|s| &s.0) != Some(&set) {
                sets.push((set, Vec::new()));
            }
            let list: Vec<String> = serde_json::from_str(&aspects).unwrap();
            if let [only] = list.as_slice() {
                let seen = &mut sets.last_mut().unwrap().1;
                if !seen.contains(only) {
                    seen.push(only.clone());
                }
            }
        }
        sets.into_iter().map(|s| s.1).collect()
    }

    #[test]
    fn aspect_order_constant_matches_the_set_numbering() {
        let f = aspect_fixture();
        let orders = derived_set_orders(&f.conn);
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0], ASPECT_ORDER, "a full set walks the whole order");
        // A set missing an aspect keeps the rest in the same relative order.
        let mut rest = ASPECT_ORDER.iter();
        for a in &orders[1] {
            assert!(rest.any(|o| o == a), "{a} is out of order in {orders:?}");
        }
    }

    #[test]
    fn order_by_aspects_sorts_the_list_then_cost_with_unaspected_last() {
        let f = aspect_fixture();
        let want_asc = [
            "vv-p", "vh-p", "v2-p", "v5-p", "v7-p", "ch-p", "c3-p", "ah-p", "a4-p", "a6-p", "u3-p",
            "u6-p", "vl-p", "hv-p", "h1-p", "ldr-p", "none-p",
        ];
        assert_eq!(ordered(&f, "aspects", "asc"), want_asc);
        // Descending reverses the aspect lists; unaspected stay last.
        let want_desc = [
            "h1-p", "ldr-p", "hv-p", "vl-p", "u3-p", "u6-p", "a4-p", "a6-p", "ah-p", "c3-p",
            "ch-p", "v2-p", "v5-p", "v7-p", "vh-p", "vv-p", "none-p",
        ];
        assert_eq!(ordered(&f, "aspects", "desc"), want_desc);
    }

    #[test]
    fn order_by_aspects_compares_the_printed_list_not_a_set() {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            testkit::set_on(&c, "PR1", "Printed One", "2024-01-01");
            // (card id, name, aspects), as printed on the collection screen.
            let cards: &[(&str, &str, &str)] = &[
                (
                    "jango",
                    "Jango Fett",
                    r#"["Cunning","Vigilance","Villainy"]"#,
                ),
                ("han", "Han Solo", r#"["Aggression","Heroism"]"#),
                ("haymaker", "Haymaker", r#"["Command"]"#),
                ("c3po", "C-3PO", r#"["Command"]"#),
                ("leia", "Leia Organa", r#"["Vigilance","Heroism"]"#),
                ("ig88", "IG-88", r#"["Vigilance","Aggression","Villainy"]"#),
            ];
            for (id, name, aspects) in cards {
                testkit::card_full(
                    &c,
                    id,
                    name,
                    None,
                    "Unit",
                    Some(3),
                    None,
                    None,
                    aspects,
                    "[]",
                    "[]",
                    None,
                );
            }
            for (printing, card, number, finish) in [
                ("jango-p", "jango", "1", "Normal"),
                ("han-p", "han", "2", "Normal"),
                ("haymaker-p", "haymaker", "3", "Normal"),
                ("c3po-p", "c3po", "4", "Normal"),
                ("leia-p", "leia", "5", "Normal"),
                ("ig88-p", "ig88", "6", "Normal"),
                ("ig88-pf", "ig88", "6", "Foil"),
            ] {
                testkit::printing(&c, printing, card, "PR1", number, "Standard", finish);
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let f = Fix {
            _dir: dir,
            shared,
            registry: search_meta::load_registry(&conn).unwrap(),
            flags: search_meta::load_flags(&conn).unwrap(),
            conn,
        };
        assert_eq!(
            ordered(&f, "aspects", "asc"),
            [
                "ig88-p",
                "ig88-pf",
                "leia-p",
                "c3po-p",
                "haymaker-p",
                "han-p",
                "jango-p"
            ]
        );
    }

    // --- sort=set with NULL set_sort_order (db-hdwl) ----------------------
    //
    // In prod sort_order is the default for every set — ingest never writes it.
    // The sort must therefore use release_date + set_code, both of which ingest
    // does populate.  The fixture below is shaped like prod: set_sort_order NULL
    // throughout, two sets sharing a release_date, and one set with no
    // release_date at all.

    /// A minimal fixture shaped like the prod catalog for the set-sort bug:
    /// set_sort_order = NULL everywhere, two sets share a release_date, one
    /// set has no release_date.
    fn set_sort_fixture() -> Fix {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            // Four sets.  sort_order deliberately omitted so it is the default.
            // 'shared' and 'also' share 2000-06-01; 'nodate' has no release_date.
            c.execute(
                "INSERT INTO sets (set_code, name, release_date)
                 VALUES ('early',  'Early Set',  '2000-01-01'),
                        ('shared', 'Shared Date','2000-06-01'),
                        ('also',   'Also Shared','2000-06-01'),
                        ('nodate', 'No Date Set', NULL)",
                [],
            )
            .unwrap();
            // Two cards per set, collector numbers 1 and 2.
            for (set, name) in [
                ("early", "E"),
                ("shared", "S"),
                ("also", "A"),
                ("nodate", "N"),
            ] {
                for n in ["1", "2"] {
                    let card_id = format!("{set}-{n}");
                    testkit::card(&c, &card_id, &format!("{name}-{n}"));
                    testkit::printing(
                        &c,
                        &format!("{card_id}-n"),
                        &card_id,
                        set,
                        n,
                        "Standard",
                        "Normal",
                    );
                }
            }
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        let registry = search_meta::load_registry(&conn).unwrap();
        let flags = search_meta::load_flags(&conn).unwrap();
        Fix {
            _dir: dir,
            shared,
            conn,
            registry,
            flags,
        }
    }

    /// Positive control: verifies the test fixture actually catches the bug that
    /// existed in prod.  With the old `s.set_sort_order` sort key (always NULL),
    /// SQLite falls back to the tiebreak `NUMBER_KEY, p.printing_id`,
    /// which interleaves sets by collector number instead of grouping them.
    ///
    /// This test would fail on the pre-fix code and passes after the fix.
    #[test]
    fn sort_by_set_groups_each_set_as_a_contiguous_run_asc() {
        let f = set_sort_fixture();
        let mut c = all_printings();
        c.set_catalog_wide(true);
        c.override_order(Some("set"), Some("asc"));
        let ids: Vec<String> = search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.set_code)
            .collect();
        // Each set must form a contiguous run — no set_code interleaves another.
        let runs: Vec<&str> = {
            let mut v: Vec<&str> = Vec::new();
            for code in &ids {
                if v.last().copied() != Some(code.as_str()) {
                    v.push(code.as_str());
                }
            }
            v
        };
        assert_eq!(
            runs.len(),
            4,
            "four distinct contiguous runs expected (one per set), got: {runs:?}"
        );
        // SQLite treats NULL as less than every non-NULL value, so NULL
        // release_date sorts FIRST in ASC and LAST in DESC.
        assert_eq!(
            runs[0], "nodate",
            "NULL release_date sorts first (least) asc: {runs:?}"
        );
        // early set (1999) comes before the 2000/06/01 sets.
        assert_eq!(
            runs[1], "early",
            "earliest non-null date set is second asc: {runs:?}"
        );
        // Within each set, collector numbers are ascending.
        // Within the early set, collector numbers must be ascending.
        let rows = search(&f.conn, &c).unwrap();
        let raw: Vec<&str> = rows
            .iter()
            .filter(|r| r.set_code == "early")
            .map(|r| r.printing_id.as_str())
            .collect();
        assert_eq!(
            raw,
            vec!["early-1-n", "early-2-n"],
            "card 1 before 2 within set"
        );
    }

    #[test]
    fn sort_by_set_desc_reverses_set_order_and_keeps_runs() {
        let f = set_sort_fixture();
        let mut c = all_printings();
        c.set_catalog_wide(true);
        c.override_order(Some("set"), Some("desc"));
        let ids: Vec<String> = search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.set_code)
            .collect();
        let runs: Vec<&str> = {
            let mut v: Vec<&str> = Vec::new();
            for code in &ids {
                if v.last().copied() != Some(code.as_str()) {
                    v.push(code.as_str());
                }
            }
            v
        };
        assert_eq!(
            runs.len(),
            4,
            "four contiguous runs expected in desc order: {runs:?}"
        );
        // SQLite NULL < non-NULL, so in DESC the newest dates come first and
        // NULL release_date sorts LAST.
        assert_eq!(
            runs[3], "nodate",
            "NULL release_date sorts last (least) desc: {runs:?}"
        );
        // early set (1999, oldest non-null date) comes second-to-last in DESC.
        assert_eq!(
            runs[2], "early",
            "earliest non-null date set is second-to-last desc: {runs:?}"
        );
        // Within the early set, collector numbers remain ascending.
        let rows2 = search(&f.conn, &c).unwrap();
        let raw: Vec<&str> = rows2
            .iter()
            .filter(|r| r.set_code == "early")
            .map(|r| r.printing_id.as_str())
            .collect();
        assert_eq!(
            raw,
            vec!["early-1-n", "early-2-n"],
            "card 1 before 2 within set even in desc mode"
        );
    }

    /// Paging across a set boundary must not drop or duplicate any card
    /// (regression guard for pd-tjym / pd-tsqd).
    #[test]
    fn sort_by_set_paged_walk_has_no_gaps_or_repeats() {
        let f = set_sort_fixture();
        let mut c = all_printings();
        c.set_catalog_wide(true);
        c.override_order(Some("set"), Some("asc"));
        let expected: Vec<String> = search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.printing_id)
            .collect();
        let mut walked: Vec<String> = Vec::new();
        let mut offset = 0u32;
        loop {
            let page = search_page(&f.conn, &c, Slice::Page { limit: 3, offset }).unwrap();
            if page.rows.is_empty() {
                break;
            }
            walked.extend(page.rows.into_iter().map(|r| r.printing_id));
            offset += 3;
        }
        assert_eq!(
            walked, expected,
            "paged walk must equal the full result with no gaps or repeats"
        );
    }

    // --- The sort surface -------------------------------------------------
    //
    // Every key `sort=` accepts orders by a stored scalar in ONE database.
    // `value` and `adj` did not: each was a subquery joining the tenant's
    // `collection` to the shared catalog's prices across the `ATTACH` boundary,
    // which no index can span, so ordering by either had to materialise and
    // sort the whole match set before `LIMIT` could discard any of it (pd-tjym).

    /// Every advertised key has its own arm. An entry with no arm falls through
    /// to the name fallback, which is a caller being served a different order
    /// than it asked for without being told.
    #[test]
    fn every_sortable_key_orders_by_something_other_than_the_name_fallback() {
        let fallback = order_sql(None);
        assert_eq!(fallback, "cd.name");
        for key in SORT_KEYS {
            if *key == "name" {
                continue;
            }
            assert_ne!(
                order_sql(Some(key)),
                fallback,
                "`{key}` is advertised as sortable but has no arm in order_sql"
            );
        }
    }

    /// The two the epic removed. They are not sortable, and nothing quietly
    /// orders by them under another name.
    #[test]
    fn value_and_adj_are_not_sortable() {
        assert!(!is_sortable("value"));
        assert!(!is_sortable("adj"));
        assert!(!SORT_KEYS.contains(&"value"));
        assert!(!SORT_KEYS.contains(&"adj"));
    }

    /// `override_order` lowercases what it stores, so the validator a caller
    /// runs first has to agree with it or a header click that arrives
    /// capitalised is refused for a column that does sort.
    #[test]
    fn sortable_keys_are_matched_case_insensitively() {
        assert!(is_sortable("PRICE"));
        assert!(is_sortable("Qty"));
        assert!(!is_sortable("Value"));
    }

    /// Nothing in a served query reaches `conditions` any more: the condition
    /// multiplier was only ever read to order by `value`/`adj`. The table is
    /// still there and still read at render time for one card — this asserts
    /// the 56k-row ordering path does not touch it.
    #[test]
    fn no_sort_reads_the_conditions_table() {
        for key in SORT_KEYS {
            let sql = order_sql(Some(key));
            assert!(
                !sql.contains("conditions"),
                "`{key}` orders through the conditions table: {sql}"
            );
        }
        let mut c = all_printings();
        c.override_order(Some("price"), Some("desc"));
        assert!(!build_full_sql(&c, None).contains("conditions"));
    }

    /// `order:price` for a catalog printing is decided entirely inside
    /// `shared` (pd-m4gw). The newest catalog quote prices it; a tenant
    /// `manual_prices` row against the same printing — the residue a
    /// collection may still carry — cannot move it.
    #[test]
    fn order_by_price_reads_the_catalog_quote_and_not_a_tenant_manual_price() {
        let f = fixture();
        f.set_catalog_price("luke-foil", 10.0);
        f.set_catalog_price("falcon-std", 20.0);
        // Written straight to the table: `manual_prices::insert` refuses a
        // catalog printing outright.
        f.conn
            .execute(
                "INSERT INTO manual_prices (printing_id, price, observed_at) \
                 VALUES ('luke-foil', 9999.0, '2026-08-11T00:00:00Z')",
                [],
            )
            .unwrap();

        let desc = ordered(&f, "price", "desc");
        assert_eq!(
            &desc[..2],
            &["falcon-std", "luke-foil"],
            "the $9999 tenant row must not lift the Luke above the Falcon: {desc:?}"
        );
    }

    /// The ordering expression names no tenant table for a catalog row — the
    /// structural half of the assertion above. `manual_prices` is still
    /// reachable, but only behind the `user_printings` guard, so a printing
    /// the catalog knows about cannot reach across the ATTACH boundary.
    #[test]
    fn order_by_price_reaches_a_tenant_table_only_through_the_user_printings_guard() {
        let sql = order_sql(Some("price"));
        assert!(sql.contains("FROM prices lp"));
        let manual = sql
            .split_once("manual_prices")
            .expect("price still consults manual_prices for user-created printings")
            .1;
        assert!(
            manual.contains("user_printings"),
            "the tenant arm is unguarded — a catalog printing could price from \
             the tenant again: {sql}"
        );
    }

    #[test]
    fn limit_bounds_the_page_and_total_counts_the_whole_result() {
        let f = fixture();
        let c = all_printings();
        // Read the catalog size off the query rather than hardcoding it — the
        // fixture grows as other beads land, and paging is what is under test.
        let all = search(&f.conn, &c).unwrap().len() as i64;
        assert!(all > 2, "fixture needs more printings than the page below");

        let page = search_page(
            &f.conn,
            &c,
            Slice::Page {
                limit: 2,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(page.rows.len(), 2, "limit bounds the rows returned");
        assert_eq!(
            page.total, all,
            "total is the unbounded count, not the page"
        );
        assert_eq!((page.limit, page.offset), (2, 0), "page echoes its bounds");

        // A limit past the end returns what exists, not an error.
        let over = search_page(
            &f.conn,
            &c,
            Slice::Page {
                limit: 100,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(over.rows.len() as i64, all);
        assert_eq!(over.total, all);

        // limit=0 is a legal count-only request.
        let none = search_page(
            &f.conn,
            &c,
            Slice::Page {
                limit: 0,
                offset: 0,
            },
        )
        .unwrap();
        assert!(none.rows.is_empty());
        assert_eq!(none.total, all);
    }

    /// pd-7z4o. `Slice::All` is the whole result, and the envelope says so by
    /// echoing the row count as the limit — a client that windows the DOM needs
    /// to know it holds everything, not merely that it got a lot.
    #[test]
    fn slice_all_returns_every_row_and_echoes_what_it_served() {
        let f = fixture();
        let c = all_printings();
        let all = search(&f.conn, &c).unwrap().len();
        assert!(all > 2, "fixture needs more printings than a page");

        let page = search_page(&f.conn, &c, Slice::All).unwrap();
        assert_eq!(page.rows.len(), all);
        assert_eq!(page.total, all as i64);
        assert_eq!((page.limit as usize, page.offset), (all, 0));
    }

    // --- the result set's value (pd-2g84) ----------------------------------

    /// The headline claim: the money beside the count is the same fact as the
    /// count, so it is the same *shape* of fact — about the whole result set,
    /// whatever slice of it the caller asked for. A client holding one page
    /// can only sum that page, which is why this cannot be its job.
    ///
    /// Fails if `total_value` is computed over the returned rows: the fixture
    /// owns copies of two different printings, and a page of one holds only
    /// one of them.
    #[test]
    fn total_value_is_the_whole_result_whatever_slice_was_asked_for() {
        let f = fixture();
        // Two owned Lukes (Near Mint ×1.00 and Lightly Played ×0.85) and
        // one Near Mint Falcon.
        f.set_catalog_price("luke-foil", 100.0);
        f.set_catalog_price("falcon-std", 10.0);
        let expected = 100.0 * 1.00 + 100.0 * 0.85 + 10.0 * 1.00;

        let c = all_printings();
        let all = search(&f.conn, &c).unwrap().len();
        assert!(all > 2, "fixture needs more printings than the pages below");

        for slice in [
            Slice::All,
            Slice::Page {
                limit: 1,
                offset: 0,
            },
            Slice::Page {
                limit: 0,
                offset: 0,
            },
            Slice::Page {
                limit: 10,
                offset: 999,
            },
        ] {
            let page = search_page(&f.conn, &all_printings(), slice).unwrap();
            let got = page.total_value.expect("the fixture owns priced copies");
            assert!(
                (got - expected).abs() < 1e-9,
                "{slice:?}: total_value is the whole result's value \
                 (expected {expected}, got {got})"
            );
        }
    }

    /// The number is a button that opens the value-over-time chart, and that
    /// chart values `status = 'owned'` copies. So this does too — an `ordered`
    /// copy is listed by owned mode and counted by `total`, and is not money
    /// the collection holds; a `sold` one is neither.
    #[test]
    fn total_value_counts_owned_copies_and_not_the_others() {
        let f = fixture();
        f.set_catalog_price("han-std", 50.0);
        // Han Solo is unowned in the fixture. Give it one copy per status
        // owned mode is willing to list, plus a sold one.
        f.conn
            .execute(
                "INSERT INTO collection (printing_id,condition,language,acquired_at,source,status)
                 VALUES
                 ('han-std','Near Mint','English','2024-03-01','manual_id','owned'),
                 ('han-std','Near Mint','English','2024-03-02','manual_id','ordered'),
                 ('han-std','Near Mint','English','2024-03-03','manual_id','sold')",
                [],
            )
            .unwrap();

        let c = f.compile("han solo");
        let page = search_page(&f.conn, &c, Slice::All).unwrap();
        assert_eq!(page.total, 1, "one printing matches, whatever its copies");
        assert_eq!(
            page.rows[0].copies.len(),
            3,
            "all three copies are drawn — this is about what is VALUED"
        );
        let got = page.total_value.unwrap();
        assert!(
            (got - 50.0).abs() < 1e-9,
            "only the owned copy is money the collection holds, got {got}"
        );
    }

    /// A result with no owned, priced copy answers `None`, not `Some(0.0)`.
    /// "Worth nothing" and "no figure to show" are the same answer here, and
    /// `$0.00` beside a page of cards nobody has priced is a claim.
    #[test]
    fn a_result_with_nothing_owned_or_nothing_priced_has_no_value() {
        let f = fixture();
        // Nothing is priced yet, though three copies are owned.
        let priced_nothing = search_page(&f.conn, &all_printings(), Slice::All).unwrap();
        assert!(priced_nothing.total > 0, "rows matched");
        assert_eq!(
            priced_nothing.total_value, None,
            "owned copies nobody prices are not worth $0.00"
        );

        // Price everything; now ask for a result that matches only printings
        // nobody owns.
        f.set_catalog_price("luke-foil", 100.0);
        f.set_catalog_price("han-std", 50.0);
        let unowned = search_page(&f.conn, &f.compile("is:missing"), Slice::All).unwrap();
        assert!(unowned.total > 0, "catalog-wide rows matched");
        assert_eq!(
            unowned.total_value, None,
            "a result of printings nobody owns is worth nothing to this owner"
        );
    }

    /// An owned copy the catalog does not price contributes nothing — it does
    /// not zero the printing it shares with a priced copy, and it does not
    /// zero the result.
    #[test]
    fn an_unpriced_copy_costs_the_sum_nothing_rather_than_emptying_it() {
        let f = fixture();
        // Only the Luke is priced; the owned Falcon is not.
        f.set_catalog_price("luke-foil", 100.0);
        let page = search_page(&f.conn, &all_printings(), Slice::All).unwrap();
        let got = page.total_value.unwrap();
        assert!(
            (got - (100.0 + 85.0)).abs() < 1e-9,
            "the unpriced Falcon drops out of the sum, got {got}"
        );
    }

    #[test]
    fn offset_past_the_end_is_an_empty_page_with_an_honest_total() {
        let f = fixture();
        let c = all_printings();
        let all = search(&f.conn, &c).unwrap().len() as i64;
        let page = search_page(
            &f.conn,
            &c,
            Slice::Page {
                limit: 10,
                offset: 999,
            },
        )
        .unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.total, all);
    }

    // The classic form of this bug: an ORDER BY that is not a total order lets
    // SQLite return tied rows in either order between statements, so an OFFSET
    // walk drops some rows and repeats others. The fixture has a tie pair by
    // construction — falcon-std and falcon-hyper are the same card, so they
    // share `cd.name`, the first ORDER BY key, and differ only by number.
    //
    // This asserts the SQL shape rather than a walk, deliberately: SQLite's
    // sorter happens to be stable at four rows, so the walk below passes with
    // or without a unique key and cannot be the guard. Uniqueness of the sort
    // key is the property; the walk is what it buys.
    #[test]
    fn order_by_is_a_total_order() {
        let sql = build_full_sql(&all_printings(), Some((1, 0)));
        assert!(
            sql.contains(&format!(
                "{NUMBER_KEY} ASC, p.printing_id ASC LIMIT ? OFFSET ?"
            )),
            "unique final sort key + bound paging: {sql}"
        );
        // And the bounds are bound parameters, never formatted into the SQL.
        assert!(sql.ends_with("LIMIT ? OFFSET ?"), "bounds are bound: {sql}");
        assert!(build_full_sql(&all_printings(), None).ends_with("p.printing_id ASC"));
    }

    #[test]
    fn offset_walks_the_full_result_without_gaps_or_repeats() {
        let f = fixture();
        let c = all_printings();
        let expected: Vec<String> = search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.printing_id)
            .collect();

        // Walk one row at a time — the harshest paging the client can ask for.
        let mut walked: Vec<String> = Vec::new();
        let mut offset = 0u32;
        loop {
            let page = search_page(&f.conn, &c, Slice::Page { limit: 1, offset }).unwrap();
            assert_eq!(page.total, expected.len() as i64, "total is page-invariant");
            if page.rows.is_empty() {
                break;
            }
            walked.extend(page.rows.into_iter().map(|r| r.printing_id));
            offset += 1;
        }
        assert_eq!(walked, expected, "pages concatenate to the whole result");

        // Same walk at a page size that does not divide the total evenly.
        let mut walked3: Vec<String> = Vec::new();
        let mut offset = 0u32;
        while (offset as usize) < expected.len() {
            let page = search_page(&f.conn, &c, Slice::Page { limit: 3, offset }).unwrap();
            walked3.extend(page.rows.into_iter().map(|r| r.printing_id));
            offset += 3;
        }
        assert_eq!(walked3, expected);
    }

    #[test]
    fn paging_survives_a_sort_column_that_is_null_for_every_row() {
        let f = fixture();
        // `order:price` — no prices in the fixture, so every row ties on NULL
        // and only the tiebreakers separate them.
        let mut c = all_printings();
        c.override_order(Some("price"), None);
        let expected: Vec<String> = search(&f.conn, &c)
            .unwrap()
            .into_iter()
            .map(|r| r.printing_id)
            .collect();
        let mut walked: Vec<String> = Vec::new();
        for offset in 0..expected.len() as u32 {
            let page = search_page(&f.conn, &c, Slice::Page { limit: 1, offset }).unwrap();
            walked.extend(page.rows.into_iter().map(|r| r.printing_id));
        }
        assert_eq!(walked, expected, "all-NULL sort still pages coherently");
    }

    #[test]
    fn paging_respects_the_query_filter() {
        let f = fixture();
        // Owned mode, one match: paging must not widen or narrow the filter.
        let c = f.compile("a:vigilance");
        let page = search_page(
            &f.conn,
            &c,
            Slice::Page {
                limit: 10,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.rows[0].printing_id, "luke-foil");
        assert_eq!(
            page.rows[0].copies.len(),
            2,
            "copies still attach on a page"
        );
    }

    // --- the sort surface: stability + index satisfaction (pd-66hq) --------

    /// A catalog where every sort key ties across many rows at once, so the
    /// only thing separating two rows is the tiebreak: 40 names over 200
    /// cards, one rarity per 4 cards, two sets, three variants per card — 600
    /// printings where `order:name` leaves 15-row ties and `order:type`
    /// leaves one 600-row tie. The 5-row fixture has one tie pair.
    ///
    /// Honest about what the walk below can and cannot prove: SQLite's sorter
    /// is deterministic for a fixed plan and fixed data, so a paged walk and
    /// the unpaged query agree on tied rows here whether or not the tiebreak
    /// is in the ORDER BY — removing it does not turn the walk red. The
    /// tiebreak's necessity is asserted where it can be, on the SQL shape
    /// (`every_sort_key_carries_the_printing_id_tiebreak`); the walk is
    /// what that buys, and it is what catches a *plan* that differs between a
    /// window and the whole result — the thing a spot check cannot see.
    fn tie_dense_fixture() -> Fix {
        const CARDS: usize = 200;
        const NAMES: usize = 40;
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared.sqlite");
        {
            let mut c = open_shared(&shared).unwrap();
            search_meta::reconcile(&mut c).unwrap();
            testkit::set_on(&c, "TI1", "Tie Set One", "2024-01-01");
            testkit::set_on(&c, "TI2", "Tie Set Two", "2024-06-01");
            let tx = c.transaction().unwrap();
            {
                let mut card = tx
                    .prepare(
                        "INSERT INTO cards (card_id,name,card_type,cost,power,hp)
                         VALUES (?1,?2,?3,?4,?5,?6)",
                    )
                    .unwrap();
                let mut pr = tx
                    .prepare(
                        "INSERT INTO printings (printing_id,card_id,set_code,number,variant,
                             finish,rarity)
                         VALUES (?1,?2,?3,?4,?5,'Normal',?6)",
                    )
                    .unwrap();
                let mut price = tx
                    .prepare(
                        "INSERT INTO prices (printing_id,source,observed_on,market)
                         VALUES (?1,'tcgcsv','2026-08-09',?2)",
                    )
                    .unwrap();
                for i in 0..CARDS {
                    let card_id = format!("tie-{i}");
                    card.execute(rusqlite::params![
                        card_id,
                        format!("Tied Trooper {}", i % NAMES),
                        if i % 5 == 0 { "Event" } else { "Unit" },
                        // Ten buckets each: `order:cost`, `order:power` and
                        // `order:hp` get long ties too rather than a total
                        // order of their own.
                        (i % 10) as i64,
                        (i % 10) as i64 * 2,
                        60 + (i % 10) as i64 * 20,
                    ])
                    .unwrap();
                    for (v, variant) in ["Standard", "Hyperspace", "Showcase"].iter().enumerate() {
                        let n = (i * 3 + v) as i64;
                        let printing_id = format!("{card_id}-{variant}");
                        pr.execute(rusqlite::params![
                            printing_id,
                            card_id,
                            if i % 2 == 0 { "TI1" } else { "TI2" },
                            // Unique per (set, variant); ties across sets.
                            (i / 2 + 1).to_string(),
                            variant,
                            ["Common", "Uncommon", "Rare", "Legendary"][(i / 4) % 4],
                        ])
                        .unwrap();
                        // Prices in 20 buckets, and every fifth printing has
                        // none at all — `order:price` must tie on NULL too.
                        if n % 5 != 0 {
                            price
                                .execute(rusqlite::params![printing_id, (i % 20) as f64 + 0.5])
                                .unwrap();
                        }
                    }
                }
            }
            tx.commit().unwrap();
        }
        let conn = connect_user(&dir.path().join("collection.sqlite"), &shared).unwrap();
        // Own 0, 1 or 2 copies of the first 50 cards' printings so `order:qty`
        // has three buckets and the rest of the catalog ties at 0.
        {
            let tx = conn.unchecked_transaction().unwrap();
            {
                let mut ins = tx
                    .prepare(
                        "INSERT INTO collection (printing_id,condition,language,acquired_at,
                             source,status)
                         VALUES (?1,?2,'English','2025-01-01','manual_id','owned')",
                    )
                    .unwrap();
                for i in 0..50 {
                    let card_id = format!("tie-{i}");
                    for (v, variant) in ["Standard", "Hyperspace", "Showcase"].iter().enumerate() {
                        let printing = format!("{card_id}-{variant}");
                        for _ in 0..(v % 3) {
                            ins.execute(rusqlite::params![
                                printing,
                                ["Near Mint", "Lightly Played"][i % 2]
                            ])
                            .unwrap();
                        }
                    }
                }
            }
            tx.commit().unwrap();
        }
        let registry = search_meta::load_registry(&conn).unwrap();
        let flags = search_meta::load_flags(&conn).unwrap();
        Fix {
            _dir: dir,
            shared,
            conn,
            registry,
            flags,
        }
    }

    /// Every sort ends in the unique `p.printing_id` tiebreak — the
    /// property the offset walk below depends on. Asserted per key rather than
    /// once for the default, because `order_sql` is a match arm per key and a
    /// new arm is exactly where the tiebreak would go missing.
    #[test]
    fn every_sort_key_carries_the_printing_id_tiebreak() {
        for key in SORT_KEYS {
            let mut c = all_printings();
            c.override_order(Some(key), Some("desc"));
            let sql = build_full_sql(&c, Some((250, 0)));
            assert!(
                sql.contains(&format!(
                    "{NUMBER_KEY} ASC, p.printing_id ASC LIMIT ? OFFSET ?"
                )),
                "order:{key} must end in the unique tiebreak: {sql}"
            );
            // The direction applies to the sort column, never to the tiebreak:
            // flipping the tiebreak with it would reorder tied rows between a
            // page and the whole result.
            // (`aspects` slots cost and name, also ASC, between the two.)
            let after = if *key == "aspects" {
                "DESC, cd.cost ASC, cd.name ASC"
            } else {
                "DESC"
            };
            assert!(
                sql.contains(&format!("{after}, {NUMBER_KEY} ASC")),
                "order:{key} desc keeps an ASC tiebreak: {sql}"
            );
        }
    }

    /// The tiebreak is only worth having if the column it names is unique —
    /// `printing_id` is the primary key of *both* tables the search reads
    /// printings from, so it is unique across their union as well.
    #[test]
    fn the_tiebreak_column_is_unique_in_every_table_it_comes_from() {
        let f = tie_dense_fixture();
        // Qualified by database: `printings` is the catalog's, reached through
        // the TEMP VIEW the ATTACH puts up, and a view has no indexes of its
        // own — asking unqualified would ask the view and always answer no.
        for (db, table) in [("shared", "printings"), ("main", "user_printings")] {
            let unique_on_printing_id: bool = f
                .conn
                .prepare(&format!(
                    "SELECT EXISTS (SELECT 1 FROM pragma_index_list('{table}','{db}') l
                        WHERE l.\"unique\" = 1
                          AND (SELECT COUNT(*) FROM pragma_index_info(l.name,'{db}')) = 1
                          AND (SELECT name FROM pragma_index_info(l.name,'{db}')) = 'printing_id')"
                ))
                .unwrap()
                .query_row([], |r| r.get(0))
                .unwrap();
            assert!(
                unique_on_printing_id,
                "{db}.{table}.printing_id must be backed by a unique index — the ORDER BY \
                 tiebreak is a total order only if it is"
            );
        }
    }

    /// Walk the ENTIRE result in windows, for every sort key, in both
    /// directions, and assert the windows concatenate to exactly the unpaged
    /// order: no row dropped, no row served twice.
    ///
    /// The window size deliberately does not divide the total, so boundaries
    /// land mid-tie — and the catalog is built so that every key has long
    /// ties for them to land in ([`tie_dense_fixture`], which also says what
    /// this walk does and does not prove).
    #[test]
    fn offset_walk_over_every_sort_key_has_no_gaps_or_repeats() {
        let f = tie_dense_fixture();
        for key in SORT_KEYS {
            for dir in ["asc", "desc"] {
                let mut c = all_printings();
                c.override_order(Some(key), Some(dir));
                let expected: Vec<String> = search(&f.conn, &c)
                    .unwrap()
                    .into_iter()
                    .map(|r| r.printing_id)
                    .collect();
                assert_eq!(expected.len(), 600, "order:{key} {dir} sees the catalog");

                let mut walked: Vec<String> = Vec::new();
                let mut offset = 0u32;
                loop {
                    let page = search_page(&f.conn, &c, Slice::Page { limit: 37, offset }).unwrap();
                    assert_eq!(
                        page.total,
                        expected.len() as i64,
                        "order:{key} {dir}: total is page-invariant"
                    );
                    if page.rows.is_empty() {
                        break;
                    }
                    walked.extend(page.rows.into_iter().map(|r| r.printing_id));
                    offset += 37;
                }
                assert_eq!(
                    walked, expected,
                    "order:{key} {dir}: windows concatenate to the whole result"
                );
                let mut unique: Vec<&String> = walked.iter().collect();
                unique.sort();
                unique.dedup();
                assert_eq!(
                    unique.len(),
                    expected.len(),
                    "order:{key} {dir}: no printing served twice"
                );
            }
        }
    }

    /// Which orderings SQLite still has to sort in a temp b-tree, and — the
    /// point of pinning it — which no longer do.
    ///
    /// A sort that falls back to a temp b-tree materialises and sorts the
    /// whole match set before `LIMIT` can discard any of it, so it is "fast
    /// today at this row count" rather than fast. `EXPLAIN QUERY PLAN` is the
    /// only thing that tells the two apart, which is why this is a test and
    /// not a review note.
    ///
    /// **Every key `sort=` accepts is still on this list** — the constant IS
    /// [`SORT_KEYS`] — and no index closes any of them, because two things block index satisfaction structurally
    /// (measured on SQLite 3.45.1, pd-66hq):
    ///
    /// 1. **The ORDER BY's unique tiebreak lives on a different table from
    ///    its sort column.** `ORDER BY cd.name, NUMBER_KEY,
    ///    p.printing_id` needs the leading terms from `cards` and the last
    ///    from `printings`; SQLite will not use an inner-loop index to
    ///    satisfy a trailing ORDER BY term, even given
    ///    `cards(name, card_id)` and
    ///    `printings(card_id, printing_id)` as covering unique indexes. It
    ///    reports `USE TEMP B-TREE FOR RIGHT PART OF ORDER BY` and sorts.
    /// 2. **`FROM` is a cross-database `UNION ALL`** (`printings` in the
    ///    shared catalog ⋃ `user_printings` in the tenant), which SQLite
    ///    cannot flatten into a join and therefore cannot index. Even with the
    ///    sort key denormalised onto the printing row, ordering over that
    ///    compound is a temp b-tree.
    ///
    /// The shape that IS clean — verified against a 74,635-printing synthetic
    /// catalog — is a compound of two *separately joined* arms, each ordered
    /// by an index on `(sort_key, printing_id)` of its own table, which SQLite
    /// answers with `MERGE (UNION ALL)` and no sort. Getting there means the
    /// sort keys become stored scalars on `printings` *and* `user_printings`
    /// and `build_full_sql` emits a compound; that is a schema and query
    /// change, not an index, and it is filed rather than smuggled in here.
    ///
    /// **This list may only get shorter.** Make a sort index-satisfied and
    /// delete its entry in the same commit; the test fails either way round,
    /// so neither a regression nor an unclaimed win can pass quietly.
    const SORTS_THAT_STILL_SORT: &[&str] = SORT_KEYS;

    #[test]
    fn query_plan_pins_which_sorts_are_still_temp_b_tree_sorted() {
        let f = tie_dense_fixture();
        let mut sorting: Vec<&str> = Vec::new();
        for key in SORT_KEYS {
            let mut c = f.compile("t:unit");
            c.set_catalog_wide(true);
            c.override_order(Some(key), None);
            let sql = build_full_sql(&c, Some((250, 0)));
            let mut params = c.params.clone();
            params.push(Value::Integer(250));
            params.push(Value::Integer(0));
            let mut stmt = f
                .conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            let plan: Vec<String> = stmt
                .query_map(params_from_iter(params.iter()), |r| r.get::<_, String>(3))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            if plan.iter().any(|line| line.contains("TEMP B-TREE")) {
                sorting.push(key);
            }
        }
        assert_eq!(
            sorting, SORTS_THAT_STILL_SORT,
            "the set of orderings SQLite sorts in a temp b-tree moved — if one \
             became index-satisfied, take it off SORTS_THAT_STILL_SORT in this \
             commit; if one regressed, it is a performance bug"
        );
    }
}
