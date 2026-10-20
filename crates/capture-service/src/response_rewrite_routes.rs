use super::{authorize, error, ApiError, Shared};
use axum::{extract::{DefaultBodyLimit, State}, http::HeaderMap, routing::get, Json, Router};
use capture_core::response_rewrite::{Config, Snapshot};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Update { config: Config, expected_version: String }

async fn snapshot(State(state): State<Shared>, headers: HeaderMap) -> Result<Json<Snapshot>, ApiError> {
    authorize(&headers, &state)?;
    Ok(Json(state.engine.response_rewrite.snapshot()))
}

async fn configure(State(state): State<Shared>, headers: HeaderMap, Json(input): Json<Update>) -> Result<Json<Snapshot>, ApiError> {
    authorize(&headers, &state)?;
    tokio::task::spawn_blocking(move || state.engine.response_rewrite.configure(input.config, &input.expected_version))
        .await.map_err(error)?.map(Json).map_err(error)
}

pub(super) fn router() -> Router<Shared> {
    Router::new().route("/response-rewrite", get(snapshot).post(configure).layer(DefaultBodyLimit::max(16 * 1024 * 1024 + 4096)))
}
