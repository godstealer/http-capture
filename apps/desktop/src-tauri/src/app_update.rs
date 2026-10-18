//! Signed desktop updates. The webview never receives updater/process permissions.
use serde::Serialize;
use std::{future::Future, sync::Mutex as StdMutex, time::Duration};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct AppUpdateState {
    // Checking must not replace the cached release while it is being installed.
    operation: Mutex<()>,
    pending: Mutex<Option<PendingUpdate>>,
    progress: StdMutex<Option<UpdateProgress>>,
}

struct PendingUpdate { update: Update, bytes: Option<Vec<u8>> }

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    current_version: String,
    configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date: Option<String>,
}

#[derive(Clone, Serialize)]
struct UpdateProgress {
    phase: &'static str,
    downloaded: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    total: Option<u64>,
}

fn configuration_reason(value: Option<&serde_json::Value>) -> Option<String> {
    let Some(value) = value else { return Some("此版本尚未配置应用更新源".into()); };
    let config: tauri_plugin_updater::Config = match serde_json::from_value(value.clone()) {
        Ok(config) => config,
        Err(error) => return Some(format!("应用更新配置无效：{error}")),
    };
    if config.pubkey.trim().is_empty() {
        return Some("此版本尚未配置更新签名公钥".into());
    }
    if config.endpoints.is_empty() {
        return Some("此版本尚未配置应用更新源".into());
    }
    if config.endpoints.iter().any(|url| url.scheme() != "https") {
        return Some("应用更新源必须使用 HTTPS".into());
    }
    // Never silently accept a release configuration that disables TLS checks.
    for name in ["dangerousInsecureTransportProtocol", "dangerousAcceptInvalidCerts", "dangerousAcceptInvalidHostnames"] {
        if value.get(name).and_then(serde_json::Value::as_bool) == Some(true) {
            return Some("应用更新不能禁用 TLS 安全校验".into());
        }
    }
    None
}

#[tauri::command]
pub fn app_update_status(app: AppHandle) -> UpdateStatus {
    let reason = configuration_reason(app.config().plugins.0.get("updater"));
    #[cfg(target_os = "linux")]
    let reason = reason.or_else(|| linux_installation_reason(
        tauri::utils::platform::bundle_type(),
        std::env::var_os("APPIMAGE").is_some_and(|value| !value.is_empty()),
    ));
    UpdateStatus { current_version: app.package_info().version.to_string(), configured: reason.is_none(), reason }
}

#[cfg(any(target_os = "linux", test))]
fn linux_installation_reason(bundle: Option<tauri::utils::config::BundleType>, appimage_present: bool) -> Option<String> {
    if matches!(bundle, None | Some(tauri::utils::config::BundleType::AppImage)) && !appimage_present {
        Some("解压运行的 Linux 便携版需要手动更新；请下载新版，或直接运行 AppImage 后使用应用内更新".into())
    } else { None }
}

fn emit_progress(app: &AppHandle, state: &AppUpdateState, phase: &'static str, downloaded: u64, total: Option<u64>) {
    let progress = UpdateProgress { phase, downloaded, total };
    *state.progress.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(progress.clone());
    let _ = app.emit("app-update-progress", progress);
}

fn emit_phase(app: &AppHandle, phase: &'static str) {
    let state = app.state::<AppUpdateState>();
    let previous = state.progress.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
    let (downloaded, total) = previous.map(|p| (p.downloaded, p.total)).unwrap_or((0, None));
    emit_progress(app, &state, phase, downloaded, total);
}

#[tauri::command]
pub async fn check_app_update(app: AppHandle, state: State<'_, AppUpdateState>) -> Result<Option<UpdateInfo>, String> {
    let _operation = state.operation.try_lock().map_err(|_| "正在检查或安装更新，请稍后重试")?;
    if let Some(reason) = app_update_status(app.clone()).reason { return Err(reason); }
    // A failed new check must not leave a previous offer silently installable.
    *state.pending.lock().await = None;
    let exit_app = app.clone();
    let updater = app.updater_builder()
        .timeout(Duration::from_secs(30))
        .configure_client(|client| client.https_only(true))
        .on_before_exit(move || emit_phase(&exit_app, "restarting"))
        .build().map_err(|error| format!("无法初始化应用更新：{error}"))?;
    let update = updater.check().await.map_err(|error| format!("检查应用更新失败：{error}"))?;
    if update.as_ref().is_some_and(|update| update.download_url.scheme() != "https") {
        return Err("更新安装包地址必须使用 HTTPS".into());
    }
    let info = update.as_ref().map(|update| UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        notes: update.body.clone(),
        date: update.date.and_then(|date| date.format(&time::format_description::well_known::Rfc3339).ok()),
    });
    *state.pending.lock().await = update.map(|update| PendingUpdate { update, bytes: None });
    Ok(info)
}

#[derive(Debug, PartialEq)]
enum InstallFailure { Verification(String), Cleanup(String), Install(String) }

// Keep the security/lifecycle boundary testable without launching an installer.
// Constructing the cleanup future has no side effects; it is polled only after
// the official signature verifier has accepted the cached bytes.
async fn run_install<D, C, I, F>(verify: D, cleanup: C, install: I) -> Result<(), InstallFailure>
where
    D: Future<Output = Result<Vec<u8>, String>>,
    C: Future<Output = Result<(), String>>,
    I: FnOnce(Vec<u8>) -> F,
    F: Future<Output = Result<(), String>>,
{
    let bytes = verify.await.map_err(InstallFailure::Verification)?;
    cleanup.await.map_err(InstallFailure::Cleanup)?;
    install(bytes).await.map_err(InstallFailure::Install)
}

#[tauri::command]
pub async fn download_app_update(app: AppHandle, state: State<'_, AppUpdateState>) -> Result<(), String> {
    let _operation = state.operation.try_lock().map_err(|_| "正在检查或安装更新，请勿重复操作")?;
    if let Some(reason) = app_update_status(app.clone()).reason { return Err(reason); }
    let mut update = {
        let mut pending = state.pending.lock().await;
        let pending = pending.as_mut().ok_or("请先检查并选择可用的应用更新")?;
        pending.bytes = None;
        pending.update.clone()
    };
    // A large installer can take longer than the manifest check.
    update.timeout = Some(Duration::from_secs(15 * 60));
    let mut downloaded = 0u64;
    emit_progress(&app, &state, "downloading", 0, None);
    let bytes = update.download(|chunk, content_length| {
        downloaded = downloaded.saturating_add(chunk as u64);
        emit_progress(&app, &state, "downloading", downloaded, content_length);
    }, || {}).await.map_err(|error| format!("下载或验证更新失败，抓包仍保持运行：{error}"))?;
    // The operation lock prevents check() from replacing this release meanwhile.
    state.pending.lock().await.as_mut().ok_or("更新状态已失效，请重新检查")?.bytes = Some(bytes);
    Ok(())
}

#[tauri::command]
pub async fn install_app_update(app: AppHandle, state: State<'_, AppUpdateState>, network: State<'_, crate::AppState>) -> Result<(), String> {
    let _operation = state.operation.try_lock().map_err(|_| "正在检查或安装更新，请勿重复操作")?;
    if let Some(reason) = app_update_status(app.clone()).reason { return Err(reason); }
    let (update, bytes) = {
        let mut pending = state.pending.lock().await;
        let pending = pending.as_mut().ok_or("请先检查并下载可用的应用更新")?;
        let bytes = pending.bytes.take().ok_or("请先完成更新下载和签名验证")?;
        (pending.update.clone(), bytes)
    };
    let verifier = update.clone();
    let install_app = app.clone();
    let result = run_install(
        async {
            tauri::async_runtime::spawn_blocking(move || {
                verifier.verify(&bytes).map_err(|error| error.to_string())?;
                Ok(bytes)
            }).await.map_err(|error| format!("验证任务异常：{error}"))?
        },
        network.shutdown_network(),
        move |bytes| async move {
            // Windows install() bypasses RunEvent::Exit, so cleanup is already done.
            emit_phase(&install_app, "installing");
            tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await
                .map_err(|error| format!("安装任务异常：{error}"))?
                .map_err(|error| error.to_string())
        },
    ).await;
    match result {
        Ok(()) => {},
        Err(InstallFailure::Verification(error)) => {
            return Err(format!("验证更新失败，抓包仍保持运行：{error}"));
        }
        Err(InstallFailure::Cleanup(error)) => {
            network.resume_network_commands().await.map_err(|resume|format!("更新清理失败：{error}；恢复网络操作失败：{resume}"))?;
            return Err(format!("更新已验证，但网络清理失败，尚未安装：{error}"));
        }
        Err(InstallFailure::Install(error)) => {
            network.resume_network_commands().await.map_err(|resume|format!("安装更新失败：{error}；恢复网络操作失败：{resume}"))?;
            return Err(format!("安装更新失败，应用仍在运行；可重新启动抓包：{error}"));
        }
    }
    *state.pending.lock().await = None;
    // Windows exits in install() and the installer relaunches the app by default.
    // macOS/Linux return here after replacement and need an explicit restart.
    emit_phase(&app, "restarting");
    app.restart();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unavailable_or_insecure_configuration_is_not_reported_as_ready() {
        assert!(configuration_reason(None).is_some());
        assert!(configuration_reason(Some(&json!({"pubkey":"", "endpoints":["https://example.test/update"]}))).is_some());
        assert!(configuration_reason(Some(&json!({"pubkey":"test", "endpoints":[]}))).is_some());
        assert!(configuration_reason(Some(&json!({"pubkey":"test", "endpoints":["http://example.test/update"]}))).is_some());
        assert!(configuration_reason(Some(&json!({"pubkey":"test", "endpoints":["https://example.test/update"], "dangerousAcceptInvalidCerts":true}))).is_some());
        assert!(configuration_reason(Some(&json!({"pubkey":"test", "endpoints":["https://example.test/update"]}))).is_none());
    }

    #[tokio::test]
    async fn update_operation_rejects_concurrent_attempts_and_releases_after_failure() {
        let state = AppUpdateState::default();
        let operation = state.operation.try_lock().unwrap();
        assert!(state.operation.try_lock().is_err());
        drop(operation);
        assert!(state.operation.try_lock().is_ok());
        assert!(state.pending.lock().await.is_none());
    }

    #[tokio::test]
    async fn signature_verification_failure_leaves_network_running() {
        for reason in ["download failed", "signature mismatch"] {
            let events = StdMutex::new(Vec::new());
            let result = run_install(
                async { events.lock().unwrap().push("download"); Err(reason.to_string()) },
                async { events.lock().unwrap().push("cleanup"); Ok(()) },
                |_| async { events.lock().unwrap().push("install"); Ok(()) },
            ).await;
            assert_eq!(result, Err(InstallFailure::Verification(reason.into())));
            assert_eq!(*events.lock().unwrap(), ["download"]);
        }
    }

    #[tokio::test]
    async fn cleanup_failure_prevents_installation() {
        let events = StdMutex::new(Vec::new());
        let result = run_install(
            async { events.lock().unwrap().push("verified"); Ok(vec![1]) },
            async { events.lock().unwrap().push("cleanup"); Err("TUN still exiting".into()) },
            |_| async { events.lock().unwrap().push("install"); Ok(()) },
        ).await;
        assert_eq!(result, Err(InstallFailure::Cleanup("TUN still exiting".into())));
        assert_eq!(*events.lock().unwrap(), ["verified", "cleanup"]);
    }

    #[tokio::test]
    async fn installer_failure_is_reported_after_verified_download_and_cleanup() {
        let events = StdMutex::new(Vec::new());
        let install_events = &events;
        let result = run_install(
            async { events.lock().unwrap().push("verified"); Ok(vec![1, 2, 3]) },
            async { events.lock().unwrap().push("cleanup"); Ok(()) },
            |bytes| async move { assert_eq!(bytes, [1, 2, 3]); install_events.lock().unwrap().push("install"); Err("installer unavailable".into()) },
        ).await;
        assert_eq!(result, Err(InstallFailure::Install("installer unavailable".into())));
        assert_eq!(*events.lock().unwrap(), ["verified", "cleanup", "install"]);
    }

    #[test]
    fn extracted_appimage_requires_manual_update_but_installed_formats_remain_supported() {
        use tauri::utils::config::BundleType;
        assert!(linux_installation_reason(Some(BundleType::AppImage), false).is_some());
        assert!(linux_installation_reason(Some(BundleType::AppImage), true).is_none());
        assert!(linux_installation_reason(None, false).is_some());
        assert!(linux_installation_reason(None, true).is_none());
        assert!(linux_installation_reason(Some(BundleType::Deb), false).is_none());
        assert!(linux_installation_reason(Some(BundleType::Rpm), false).is_none());
    }
}
