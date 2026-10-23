//! `/api/decks` — deck CRUD (PLAN.md §5.2, §7).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;

use dumpstar_core::deck::DeckEntry;
use dumpstar_db::DbError;
use dumpstar_db::collection::{self, CollectionRow};
use dumpstar_db::decks::{
    self, Deck, DeckEdit, DeckLineState, NewDeck, SetLines, ShortLine, Validation,
};

use crate::{AppError, AppState, blocking, blocking_read};

/// Build the deck routes (mounted under `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/decks", get(list).post(create))
        .route("/decks/{id}", get(detail).put(update).delete(remove))
        .route("/decks/{id}/lines", get(lines).put(set_lines))
        .route("/decks/{id}/release", post(release))
        .route("/decks/{id}/validate", get(validate))
        .route("/card-counts", get(counts))
}

/// A deck's lines and the copies backing them.
#[derive(Serialize)]
struct LinesBody {
    lines: Vec<DeckLineState>,
}

async fn lines(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<LinesBody>, AppError> {
    let lines = blocking_read(&state, move |c| {
        decks::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck {id}")))?;
        decks::line_states(c, id)
    })
    .await?;
    Ok(Json(LinesBody { lines }))
}

#[derive(serde::Deserialize)]
struct SetLinesBody {
    lines: Vec<DeckEntry>,
}

/// Refused set-lines: the lines that could not be filled, by name.
#[derive(Serialize)]
struct ShortBody {
    error: String,
    short: Vec<ShortLine>,
}

#[derive(Serialize)]
struct SetLinesDone {
    reserved: u32,
    lines: Vec<DeckLineState>,
    validation: Validation,
}

/// Replace the deck's lines in one transaction, reserving owned copies.
/// 409 with the short lines named when not enough copies are free.
async fn set_lines(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<SetLinesBody>,
) -> Result<Response, AppError> {
    let out = blocking(&state, move |c| {
        Ok(match decks::set_lines(c, id, &body.lines)? {
            SetLines::Short(short) => Err(short),
            SetLines::Set { reserved } => Ok(SetLinesDone {
                reserved,
                lines: decks::line_states(c, id)?,
                validation: decks::validate(c, id)?,
            }),
        })
    })
    .await?;
    Ok(match out {
        Ok(done) => Json(done).into_response(),
        Err(short) => {
            let names: Vec<String> = short.iter().map(|s| s.card_id.clone()).collect();
            let error = format!("not enough free copies for: {}", names.join(", "));
            (StatusCode::CONFLICT, Json(ShortBody { error, short })).into_response()
        }
    })
}

#[derive(Serialize)]
struct Released {
    released: u32,
}

/// Free the deck's copies; its lines stay.
async fn release(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Released>, AppError> {
    let released = blocking(&state, move |c| decks::release(c, id)).await?;
    Ok(Json(Released { released }))
}

async fn validate(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<Validation>, AppError> {
    Ok(Json(
        blocking_read(&state, move |c| decks::validate(c, id)).await?,
    ))
}

#[derive(serde::Deserialize)]
struct CountsQuery {
    /// Comma-separated game card ids; absent lists every owned card.
    ids: Option<String>,
}

/// Owned, reserved and available copies per game card.
async fn counts(
    State(state): State<AppState>,
    Query(q): Query<CountsQuery>,
) -> Result<Json<Vec<decks::CardCount>>, AppError> {
    let ids: Option<Vec<String>> = q.ids.map(|s| {
        s.split(',')
            .filter(|i| !i.is_empty())
            .map(String::from)
            .collect()
    });
    Ok(Json(
        blocking_read(&state, move |c| decks::card_counts(c, ids.as_deref())).await?,
    ))
}

/// A deck plus the cards assigned to it.
#[derive(Serialize, ts_rs::TS)]
#[ts(export)]
struct DeckDetail {
    deck: Deck,
    cards: Vec<CollectionRow>,
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Deck>>, AppError> {
    Ok(Json(blocking_read(&state, |c| decks::list(c)).await?))
}

async fn create(
    State(state): State<AppState>,
    Json(new): Json<NewDeck>,
) -> Result<(StatusCode, Json<Deck>), AppError> {
    let deck = blocking(&state, move |c| {
        let id = decks::create(c, &new)?;
        decks::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck {id}")))
    })
    .await?;
    Ok((StatusCode::CREATED, Json(deck)))
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<DeckDetail>, AppError> {
    let detail = blocking_read(&state, move |c| {
        let deck = decks::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck {id}")))?;
        let cards = collection::list_by_deck(c, id)?;
        Ok(DeckDetail { deck, cards })
    })
    .await?;
    Ok(Json(detail))
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(edit): Json<DeckEdit>,
) -> Result<Json<Deck>, AppError> {
    let deck = blocking(&state, move |c| {
        if !decks::update(c, id, &edit)? {
            return Err(DbError::NotFound(format!("deck {id}")));
        }
        decks::get(c, id)?.ok_or_else(|| DbError::NotFound(format!("deck {id}")))
    })
    .await?;
    Ok(Json(deck))
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    blocking(&state, move |c| {
        if decks::delete(c, id)? {
            Ok(())
        } else {
            Err(DbError::NotFound(format!("deck {id}")))
        }
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
