use crate::AppState;
use capture_core::response_rewrite::{Config, Snapshot};
use tauri::State;

#[tauri::command]
pub fn response_rewrite(state: State<'_, AppState>) -> Snapshot {
    state.engine.response_rewrite.snapshot()
}

#[tauri::command]
pub async fn configure_response_rewrite(state: State<'_, AppState>, config: Config, expected_version: String) -> Result<Snapshot, String> {
    let engine = state.engine.clone();
    tauri::async_runtime::spawn_blocking(move || engine.response_rewrite.configure(config, &expected_version))
        .await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}
