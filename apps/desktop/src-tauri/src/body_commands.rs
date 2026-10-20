//! Complete reads and exact file exports are explicit owner actions. Metadata
//! and body-range commands never construct an empty-body Flow placeholder.
use crate::AppState;
use capture_core::{model::Flow, store::{FlowMetadata, FlowMetadataChanges, McpBodyChunk}};
use tauri::{Manager, State};

#[tauri::command]
pub async fn flow_metadata_changes(state: State<'_, AppState>, since: Option<String>) -> Result<FlowMetadataChanges, String> {
    let store = state.engine.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.metadata_changes(since.as_deref()))
        .await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn flow_metadata(state: State<'_, AppState>, id: String, expected_version: Option<String>) -> Result<Option<FlowMetadata>, String> {
    let store = state.engine.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.metadata(&id, expected_version.as_deref()))
        .await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn get_flow(state: State<'_, AppState>, id: String, expected_version: String) -> Result<Flow, String> {
    let store = state.engine.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.get_at_revision(&id, &expected_version))
        .await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn read_flow_body(state: State<'_, AppState>, id: String, variant: String, expected_version: String, offset: u64, length: usize) -> Result<McpBodyChunk, String> {
    let store = state.engine.store.clone();
    tauri::async_runtime::spawn_blocking(move || store.read_body_at_revision(&id, &variant, &expected_version, offset, length))
        .await.map_err(|error| error.to_string())?.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_flow_body(app: tauri::AppHandle, state: State<'_, AppState>, id: String, variant: String, expected_version: String) -> Result<String, String> {
    let store = state.engine.store.clone();
    let directory = app.path().download_dir().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = directory.join(format!("flow-body-{}.bin", uuid::Uuid::new_v4()));
        store.save_body_at_revision(&id, &variant, &expected_version, &path).map_err(|error| error.to_string())?;
        Ok(path.to_string_lossy().into_owned())
    }).await.map_err(|error| error.to_string())?
}
