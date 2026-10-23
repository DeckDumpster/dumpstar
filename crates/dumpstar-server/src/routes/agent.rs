//! `/api/agent` — the documents the `/mcp` tools return (`list_decks`, `get_deck`,
//! `search_cards`), over REST, so a non-agent client (`swu-cube-check`) reads decks and runs
//! searches itself instead of having an agent retype tool output into files.

use std::sync::OnceLock;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use dumpster_kit_mcp::{Keywords, ToolError};
use serde::Deserialize;
use serde_json::{Value, json};

use dumpstar_db::decks;

use crate::{AppError, AppState, blocking_read, mcp};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent/me", get(me))
        .route("/agent/decks", get(list))
        .route("/agent/decks/{id}", get(deck))
        .route("/agent/search", get(search))
}

fn tool_error(e: ToolError) -> AppError {
    match e {
        ToolError::Failed(m) => AppError::bad_request(json!({ "error": m }).to_string()),
        ToolError::Internal(m) => AppError(StatusCode::INTERNAL_SERVER_ERROR, m),
    }
}

/// Whose tenant the bearer token is served as: the cheap authenticated call `swu-cube-check
/// login --status` makes.
async fn me() -> Result<Json<Value>, AppError> {
    let id = crate::current_tenant()?;
    Ok(Json(json!({ "tenant": id.as_str() })))
}

#[derive(Deserialize)]
struct ListParams {
    group: Option<String>,
    format: Option<String>,
}

async fn list(
    State(state): State<AppState>,
    Query(p): Query<ListParams>,
) -> Result<Json<Value>, AppError> {
    let decks = blocking_read(&state, move |c| decks::list(c)).await?;
    let decks: Vec<_> = decks
        .into_iter()
        .filter(|d| {
            p.group
                .as_deref()
                .is_none_or(|g| d.group.as_deref() == Some(g))
        })
        .filter(|d| {
            p.format
                .as_deref()
                .is_none_or(|f| d.format.as_deref() == Some(f))
        })
        .collect();
    Ok(Json(json!({ "decks": decks })))
}

async fn deck(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Value>, AppError> {
    let out = blocking_read(&state, move |c| Ok(mcp::deck_response(c, id))).await?;
    Ok(Json(out.map_err(tool_error)?))
}

#[derive(Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
    limit: Option<u64>,
    cursor: Option<String>,
    owned_only: Option<bool>,
    expand_printings: Option<bool>,
}

fn keywords() -> &'static Keywords {
    static K: OnceLock<Keywords> = OnceLock::new();
    K.get_or_init(|| {
        Keywords::parse(&dumpstar_db::search_meta::keywords_json())
            .expect("the keyword registry is valid JSON")
    })
}

async fn search(
    State(state): State<AppState>,
    Query(p): Query<SearchParams>,
) -> Result<Json<Value>, AppError> {
    let args = json!({
        "q": p.q, "limit": p.limit, "cursor": p.cursor,
        "owned_only": p.owned_only, "expand_printings": p.expand_printings,
    });
    let (registry, flags) = (state.registry.clone(), state.flags.clone());
    let out = blocking_read(&state, move |c| {
        Ok(mcp::search_cards_value(
            c,
            &registry,
            &flags,
            keywords(),
            &args,
        ))
    })
    .await?;
    Ok(Json(out.map_err(tool_error)?))
}
