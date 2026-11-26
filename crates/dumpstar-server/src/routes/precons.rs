//! `/api/precons` — the preconstructed decks, and adding one.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};

use dumpstar_db::DbError;
use dumpstar_db::decks::{self, Deck};
use dumpstar_db::precons::{self, Precon};

use crate::{AppError, AppState, blocking, blocking_read};

/// Build the preconstructed-deck routes (mounted under `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/precons", get(list))
        .route("/precons/{id}/add", post(add))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<Precon>>, AppError> {
    Ok(Json(blocking_read(&state, |c| precons::list(c)).await?))
}

#[derive(serde::Deserialize)]
struct AddBody {
    /// Add it even though this tenant already has: a second deck, with copies
    /// of its own.
    #[serde(default)]
    confirm_repeat: bool,
}

/// The new deck; 409, naming the earlier deck, for an unconfirmed repeat.
async fn add(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddBody>,
) -> Result<(StatusCode, Json<Deck>), AppError> {
    let deck = blocking(&state, move |c| {
        let added = precons::add(c, &id, body.confirm_repeat)?;
        decks::get(c, added.deck_id)?
            .ok_or_else(|| DbError::NotFound(format!("deck {}", added.deck_id)))
    })
    .await?;
    Ok((StatusCode::CREATED, Json(deck)))
}
