//! `/api/sets` — the set catalog and binder pages (PLAN.md §5.2, §6).

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;

use dumpstar_db::DbError;
use dumpstar_db::art;
use dumpstar_db::binder::{self, BinderPage, BinderQuery};
use dumpstar_db::search_meta::{self, RarityMark};
use dumpstar_db::sets::{self, SetAnalytics, SetSummary};

use crate::{AppError, AppState, blocking_read};

/// Build the set-catalog routes (mounted under `/api`).
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/rarities", get(rarities))
        .route("/sets", get(list))
        .route("/sets/{code}/binder", get(binder_page))
        .route("/sets/{code}/analytics", get(analytics))
}

/// The official rarity marks the catalog apply stored.
async fn rarities(State(state): State<AppState>) -> Result<Json<Vec<RarityMark>>, AppError> {
    Ok(Json(
        blocking_read(&state, |c| search_meta::load_rarity_marks(c)).await?,
    ))
}

async fn list(State(state): State<AppState>) -> Result<Json<Vec<SetSummary>>, AppError> {
    let mut summaries = blocking_read(&state, |c| sets::list_sets(c)).await?;
    for s in &mut summaries {
        let art = art::resolve(&state.art_dir(), &s.set_code);
        s.logo_url = art.logo_url;
        s.art_url = art.crop_url;
    }
    Ok(Json(summaries))
}

async fn analytics(
    State(state): State<AppState>,
    Path(code): Path<String>,
) -> Result<Json<SetAnalytics>, AppError> {
    let stats = blocking_read(&state, move |c| {
        sets::analytics(c, &code)?.ok_or_else(|| DbError::NotFound(format!("set {code}")))
    })
    .await?;
    Ok(Json(stats))
}

#[derive(Deserialize)]
struct BinderParams {
    page: Option<i64>,
    layout: Option<i64>,
    hyperspace: Option<bool>,
    showcase: Option<bool>,
    promos: Option<bool>,
    /// `number` | `number_desc` | `price` | `name` | `rarity`.
    sort: Option<String>,
    /// In-set card-name search.
    q: Option<String>,
    /// `all` | `have` | `need` | `dupes`.
    filter: Option<String>,
}

async fn binder_page(
    State(state): State<AppState>,
    Path(code): Path<String>,
    Query(p): Query<BinderParams>,
) -> Result<Json<BinderPage>, AppError> {
    let query = BinderQuery {
        page: p.page.unwrap_or(1),
        layout: p.layout.unwrap_or(9),
        include_hyperspace: p.hyperspace.unwrap_or(true),
        include_showcase: p.showcase.unwrap_or(true),
        include_promos: p.promos.unwrap_or(true),
        sort: p.sort.unwrap_or_else(|| "number".into()),
        search: p.q.unwrap_or_default(),
        filter: p.filter.unwrap_or_else(|| "all".into()),
    };
    let mut page = blocking_read(&state, move |c| {
        binder::get_binder_page(c, &code, &query)?
            .ok_or_else(|| DbError::NotFound(format!("set {code}")))
    })
    .await?;
    let art = art::resolve(&state.art_dir(), &page.set.set_code);
    page.set.logo_url = art.logo_url;
    page.set.art_url = art.crop_url;
    Ok(Json(page))
}
