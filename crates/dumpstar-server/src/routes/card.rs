//! `/api/card` — card-detail lookups (PLAN.md §5.2).

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use dumpstar_db::DbError;
use dumpstar_db::cards::{self, CardDetail, PriceSeries};

use crate::{AppError, AppState, blocking_read};

/// Build the card-lookup routes (mounted under `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/card/{set}/{number}", get(card_detail))
        .route("/card/{set}/{number}/prices", get(card_prices))
        .route("/cards/by-set-cn", get(by_set_cn))
}

/// `?variant=Hyperspace` addresses that variant's printing at a number; without
/// it the base printing answers.
#[derive(Deserialize)]
struct VariantQuery {
    variant: Option<String>,
}

async fn lookup(
    state: &AppState,
    set: String,
    number: String,
    variant: Option<String>,
) -> Result<Json<CardDetail>, AppError> {
    let detail = blocking_read(state, move |c| {
        cards::get_card_detail_variant(c, &set, &number, variant.as_deref())?
            .ok_or_else(|| DbError::NotFound(format!("card {set}/{number}")))
    })
    .await?;
    Ok(Json(detail))
}

async fn card_detail(
    State(state): State<AppState>,
    Path((set, number)): Path<(String, String)>,
    Query(q): Query<VariantQuery>,
) -> Result<Json<CardDetail>, AppError> {
    lookup(&state, set, number, q.variant).await
}

/// Time-series price history for every printing of a card. Drives the
/// chart on the card-detail view. v1 emits one series per printing,
/// price_type=`market`; the response shape is condition-agnostic so v2
/// can add per-condition series without breaking the client.
async fn card_prices(
    State(state): State<AppState>,
    Path((set, number)): Path<(String, String)>,
    Query(q): Query<VariantQuery>,
) -> Result<Json<Vec<PriceSeries>>, AppError> {
    let series = blocking_read(&state, move |c| {
        cards::get_card_prices_variant(c, &set, &number, q.variant.as_deref())
    })
    .await?;
    Ok(Json(series))
}

#[derive(Deserialize)]
struct BySetCn {
    set: String,
    cn: String,
}

async fn by_set_cn(
    State(state): State<AppState>,
    Query(q): Query<BySetCn>,
) -> Result<Json<CardDetail>, AppError> {
    lookup(&state, q.set, q.cn, None).await
}
