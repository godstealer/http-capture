use super::AppState;
use capture_core::config_migration::{self as migration,ExportOptions,Exported,Preview,Imported};
use std::collections::BTreeMap;
use tauri::{AppHandle,State};

#[tauri::command]
pub fn config_status(state:State<'_,AppState>)->Result<serde_json::Value,String>{migration::status(&state.engine.store).map_err(|e|format!("{e:#}"))}

#[tauri::command]
pub async fn config_export(app:AppHandle,state:State<'_,AppState>,preferences:BTreeMap<String,String>,options:ExportOptions)->Result<Exported,String>{
    let store=state.engine.store.clone();let version=app.package_info().version.to_string();
    tauri::async_runtime::spawn_blocking(move||migration::export(&store,preferences,options,&version)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))
}
#[tauri::command]
pub async fn config_preview(state:State<'_,AppState>,document:String)->Result<Preview,String>{
    let store=state.engine.store.clone();tauri::async_runtime::spawn_blocking(move||migration::preview(&store,&document)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))
}
#[tauri::command]
pub async fn config_import(app:AppHandle,state:State<'_,AppState>,document:String,sections:Vec<String>,expected_revision:String,preferences:Option<BTreeMap<String,String>>)->Result<Imported,String>{
    let _migration=state.configuration_migration.lock().await;
    let store=state.engine.store.clone();let version=app.package_info().version.to_string();
    // Validate source, target revision and complete recovery point before any
    // shutdown. A malformed file cannot interrupt the user's active capture.
    let prepared=tauri::async_runtime::spawn_blocking(move||migration::prepare_import(&store,&document,&sections,&expected_revision,preferences.unwrap_or_default(),&version)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))?;
    if let Err(error)=state.shutdown_network().await {if state.engine.store.configuration_ready().is_ok(){let _=state.resume_network_commands().await;}return Err(format!("配置未导入：{error}。已停止的监听不会自动重启，请检查后重试。"));}
    let store=state.engine.store.clone();
    let result=tauri::async_runtime::spawn_blocking(move||migration::commit_import(&store,prepared)).await.map_err(|e|e.to_string()).and_then(|r|r.map_err(|e|format!("{e:#}")));
    if result.is_err()&&state.engine.store.configuration_ready().is_ok(){let _=state.resume_network_commands().await;}
    result.map_err(|e|format!("配置未导入：{e}。配置仍保留；监听已停止，如需继续使用外部 MCP 请重启应用。"))
}
#[tauri::command]
pub async fn config_recovery(state:State<'_,AppState>)->Result<Exported,String>{let store=state.engine.store.clone();tauri::async_runtime::spawn_blocking(move||migration::recovery(&store)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))}
#[tauri::command]
pub async fn config_restart(app:AppHandle,state:State<'_,AppState>)->Result<(),String>{
    if state.engine.store.configuration_ready().is_ok(){return Err("请先完成配置导入，再重启应用".into());}
    state.shutdown_network().await?;app.restart()
}
#[tauri::command]
pub async fn config_save_file(content:String,filename:String)->Result<Option<String>,String>{
    if filename.is_empty()||filename.len()>100||!filename.ends_with(".json")||filename.starts_with('.')||!filename.bytes().all(|b|b.is_ascii_alphanumeric()||b"-_.".contains(&b)){return Err("配置文件名称无效".into());}
    if content.is_empty()||content.len()>migration::MAX_DOCUMENT_BYTES{return Err("配置文件需为 1 byte–16 MiB".into());}
    let Some(file)=rfd::AsyncFileDialog::new().set_file_name(&filename).add_filter("JSON",&["json"]).save_file().await else{return Ok(None)};
    let path=file.path().to_owned();let saved=path.to_string_lossy().into_owned();
    tauri::async_runtime::spawn_blocking(move||migration::save_atomic(&path,&content)).await.map_err(|e|e.to_string())?.map_err(|e|format!("{e:#}"))?;Ok(Some(saved))
}
