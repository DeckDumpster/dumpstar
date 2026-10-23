//! `/api/deck-groups` — deck group CRUDL and the actions on a group as a unit.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use dumpstar_db::DbError;
use dumpstar_db::deck_groups::{self, DeckGroup, DeckGroupEdit, DeleteMode, NewDeckGroup};
use dumpstar_db::decks::Deck;

use crate::{AppError, AppState, blocking, blocking_read};

/// Build the deck group routes (mounted under `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/deck-groups", get(list).post(create))
        .route("/deck-groups/{id}", get(detail).put(update).delete(remove))
        .route("/deck-groups/{id}/release", post(release))
}

/// A group plus its decks.
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
struct GroupDetail {
    group: DeckGroup,
    decks: Vec<Deck>,
}

fn detail_of(c: &rusqlite::Connection, id: i64) -> Result<GroupDetail, DbError> {
    let group =
        deck_groups::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck group {id}")))?;
    let decks = deck_groups::decks_of(c, id)?;
    Ok(GroupDetail { group, decks })
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<DeckGroup>>, AppError> {
    Ok(Json(blocking_read(&state, |c| deck_groups::list(c)).await?))
}

async fn create(
    State(state): State<AppState>,
    Json(new): Json<NewDeckGroup>,
) -> Result<(StatusCode, Json<DeckGroup>), AppError> {
    let group = blocking(&state, move |c| {
        let id = deck_groups::create(c, &new)?;
        deck_groups::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck group {id}")))
    })
    .await?;
    Ok((StatusCode::CREATED, Json(group)))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<GroupDetail>, AppError> {
    Ok(Json(
        blocking_read(&state, move |c| detail_of(c, id)).await?,
    ))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(edit): Json<DeckGroupEdit>,
) -> Result<Json<GroupDetail>, AppError> {
    let out = blocking(&state, move |c| {
        if !deck_groups::update(c, id, &edit)? {
            return Err(DbError::NotFound(format!("deck group {id}")));
        }
        detail_of(c, id)
    })
    .await?;
    Ok(Json(out))
}

#[derive(serde::Deserialize)]
struct DeleteQuery {
    /// `ungroup` keeps the group's decks; `delete` deletes them with it.
    decks: Option<String>,
}

/// Delete a group. `?decks=ungroup|delete` is required: deleting decks is
/// never a default.
async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DeleteQuery>,
) -> Result<Json<deck_groups::Deleted>, AppError> {
    let mode = q
        .decks
        .as_deref()
        .and_then(DeleteMode::parse)
        .ok_or_else(|| {
            DbError::Invalid("decks=ungroup or decks=delete says what happens to the decks".into())
        })?;
    Ok(Json(
        blocking(&state, move |c| deck_groups::delete(c, id, mode)).await?,
    ))
}

#[derive(Serialize)]
struct Released {
    released: u32,
}

/// Free the copies of every deck in the group; the decks and their lines stay.
async fn release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Released>, AppError> {
    let released = blocking(&state, move |c| deck_groups::release_all(c, id)).await?;
    Ok(Json(Released { released }))
}
