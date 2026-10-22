use std::{collections::HashMap, io::Write, sync::{Mutex, OnceLock}};
use tempfile::NamedTempFile;

struct Export { file: NamedTempFile, destination: std::path::PathBuf, offset: u64 }
static EXPORTS: OnceLock<Mutex<HashMap<String, Export>>> = OnceLock::new();
fn exports() -> &'static Mutex<HashMap<String, Export>> { EXPORTS.get_or_init(|| Mutex::new(HashMap::new())) }

#[tauri::command]
pub async fn begin_session_export(format: String) -> Result<Option<String>, String> {
    if format != "json" && format != "har" { return Err("Unsupported session format".into()); }
    let name = format!("capture.{format}");
    let Some(selected) = rfd::AsyncFileDialog::new().set_file_name(&name).add_filter(&format, &[&format]).save_file().await else { return Ok(None); };
    let destination = selected.path().to_path_buf();
    let parent = destination.parent().ok_or("Invalid export destination")?;
    let file = NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    exports().lock().map_err(|_| "Export lock poisoned")?.insert(id.clone(), Export { file, destination, offset: 0 });
    Ok(Some(id))
}

#[tauri::command]
pub fn append_session_export(id: String, offset: u64, bytes: Vec<u8>) -> Result<u64, String> {
    if bytes.len() > 65536 { return Err("Export chunk exceeds 64 KiB".into()); }
    let mut guard = exports().lock().map_err(|_| "Export lock poisoned")?;
    let export = guard.get_mut(&id).ok_or("Export session not found")?;
    if offset != export.offset { return Err("Export offset differs".into()); }
    export.file.write_all(&bytes).map_err(|error| error.to_string())?;
    export.offset = export.offset.checked_add(bytes.len() as u64).ok_or("Export size overflow")?;
    Ok(export.offset)
}

#[tauri::command]
pub fn finish_session_export(id: String, commit: bool) -> Result<Option<String>, String> {
    let export = exports().lock().map_err(|_| "Export lock poisoned")?.remove(&id).ok_or("Export session not found")?;
    if !commit { return Ok(None); }
    export.file.as_file().sync_all().map_err(|error| error.to_string())?;
    let destination = export.destination.clone();
    export.file.persist(&destination).map_err(|error| error.error.to_string())?;
    Ok(Some(destination.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_chunks_commit_exact_bytes_and_abort_cleans_draft() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("session.json");
        let id = uuid::Uuid::new_v4().to_string();
        let file = NamedTempFile::new_in(directory.path()).unwrap();
        let draft = file.path().to_path_buf();
        exports().lock().unwrap().insert(id.clone(), Export { file, destination: destination.clone(), offset: 0 });
        assert_eq!(append_session_export(id.clone(), 0, b"hello".to_vec()).unwrap(), 5);
        assert!(append_session_export(id.clone(), 0, b"duplicate".to_vec()).is_err());
        assert!(append_session_export(id.clone(), 5, vec![0; 65537]).is_err());
        assert_eq!(append_session_export(id.clone(), 5, b" world".to_vec()).unwrap(), 11);
        std::fs::write(&destination, b"old content").unwrap();
        finish_session_export(id.clone(), true).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"hello world");
        assert!(!draft.exists());
        let id = uuid::Uuid::new_v4().to_string();
        let file = NamedTempFile::new_in(directory.path()).unwrap();
        let draft = file.path().to_path_buf();
        exports().lock().unwrap().insert(id.clone(), Export { file, destination: destination.clone(), offset: 0 });
        append_session_export(id.clone(), 0, b"partial".to_vec()).unwrap();
        assert!(finish_session_export(id, false).unwrap().is_none());
        assert!(!draft.exists());
        assert_eq!(std::fs::read(destination).unwrap(), b"hello world");
    }
}
