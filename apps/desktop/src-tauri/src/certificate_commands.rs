use crate::AppState;
use capture_core::certificates::{Snapshot,Import,Change};
use tauri::State;
#[tauri::command]
pub fn certificates(state:State<'_,AppState>)->Result<Snapshot,String>{state.engine.certificates.snapshot().map_err(|e|e.to_string())}
#[tauri::command]
pub async fn import_certificate(state:State<'_,AppState>,input:Import)->Result<Snapshot,String>{
 let engine=state.engine.clone();engine.store.configuration_ready().map_err(|e|e.to_string())?;
 tauri::async_runtime::spawn_blocking(move||engine.certificates.import(input)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))
}
#[tauri::command]
pub async fn change_certificate(state:State<'_,AppState>,input:Change)->Result<Snapshot,String>{
 let engine=state.engine.clone();engine.store.configuration_ready().map_err(|e|e.to_string())?;
 tauri::async_runtime::spawn_blocking(move||engine.certificates.change(input)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))
}
