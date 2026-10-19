//! Bounded, ephemeral activity summaries. HTTP is stateless: these entries say
//! "recently active", never that a remote process is still connected.
use rmcp::model::{Implementation, ProtocolVersion};
use serde::Serialize;
use std::{sync::Mutex, time::{Duration, Instant, SystemTime, UNIX_EPOCH}};

const MAX_CLIENTS: usize = 64;
const TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientView {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol: String,
    pub transport: &'static str,
    pub first_seen: u64,
    pub last_seen: u64,
    pub requests: u64,
}
struct Entry { view: ClientView, active: Instant }
#[derive(Default)]
pub struct ConnectionRegistry {
    entries: Mutex<Vec<Entry>>,
    #[cfg(windows)] bridges: Mutex<Vec<(u32, ProcessHandle)>>,
}

impl ConnectionRegistry {
    pub fn record(&self, client: &Implementation, protocol: &ProtocolVersion) {
        let name = bounded(&client.name, 160);
        if name.is_empty() { return; }
        let version = bounded(&client.version, 80);
        let protocol = bounded(protocol.as_str(), 32);
        let now = Instant::now();
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64;
        let mut entries = self.entries.lock().unwrap_or_else(|error|error.into_inner());
        entries.retain(|entry|now.duration_since(entry.active) < TTL);
        if let Some(entry) = entries.iter_mut().find(|entry|entry.view.name == name && entry.view.version == version && entry.view.protocol == protocol) {
            entry.active = now;
            entry.view.last_seen = timestamp;
            entry.view.requests = entry.view.requests.saturating_add(1);
            return;
        }
        if entries.len() == MAX_CLIENTS {
            if let Some((index, _)) = entries.iter().enumerate().min_by_key(|(_, entry)|entry.active) { entries.remove(index); }
        }
        entries.push(Entry { active:now, view:ClientView {
            id:uuid::Uuid::new_v4().to_string(), name, version, protocol, transport:"http",
            first_seen:timestamp, last_seen:timestamp, requests:1,
        }});
    }
    pub fn snapshot(&self) -> Vec<ClientView> {
        let now=Instant::now();
        let mut entries=self.entries.lock().unwrap_or_else(|error|error.into_inner());
        entries.retain(|entry|now.duration_since(entry.active)<TTL);
        let mut result:Vec<_>=entries.iter().map(|entry|entry.view.clone()).collect();
        result.sort_by(|a,b|b.last_seen.cmp(&a.last_seen).then_with(||a.name.cmp(&b.name)));
        result
    }
    pub fn clear(&self) { self.entries.lock().unwrap_or_else(|error|error.into_inner()).clear(); }

    /// Called only after HTTP authentication. PID is merely a claim until the
    /// OS confirms that the process runs this exact executable. Holding its
    /// process handle also prevents a PID-reuse race during updater shutdown.
    pub fn record_bridge_pid(&self,pid:u32)->anyhow::Result<()> {
        anyhow::ensure!(pid>0 && pid!=std::process::id(),"Invalid MCP bridge process");
        #[cfg(windows)] {
            use windows_sys::Win32::{Foundation::WAIT_OBJECT_0,System::Threading::{OpenProcess,QueryFullProcessImageNameW,WaitForSingleObject,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_SYNCHRONIZE}};
            use std::os::windows::ffi::OsStringExt;
            let mut bridges=self.bridges.lock().unwrap_or_else(|error|error.into_inner());
            bridges.retain(|(_,handle)|unsafe {WaitForSingleObject(handle.0,0)}!=WAIT_OBJECT_0);
            if bridges.iter().any(|(saved,_)|*saved==pid) {return Ok(());}
            anyhow::ensure!(bridges.len()<MAX_CLIENTS,"Too many MCP bridge processes; close unused AI connections");
            let raw=unsafe {OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_SYNCHRONIZE,0,pid)};
            anyhow::ensure!(!raw.is_null(),"Unable to verify MCP bridge process");
            let handle=ProcessHandle(raw);
            let mut image=vec![0_u16;32768];let mut size=image.len() as u32;
            anyhow::ensure!(unsafe {QueryFullProcessImageNameW(handle.0,0,image.as_mut_ptr(),&mut size)}!=0 && size>0 && (size as usize)<=image.len(),"Unable to verify MCP bridge executable");
            let image=std::path::PathBuf::from(std::ffi::OsString::from_wide(&image[..size as usize]));
            let expected=std::env::current_exe()?.canonicalize()?;
            let image=image.canonicalize()?;
            anyhow::ensure!(image.to_string_lossy().eq_ignore_ascii_case(&expected.to_string_lossy()),"MCP bridge must run the same application executable");
            anyhow::ensure!(unsafe {WaitForSingleObject(handle.0,0)}!=WAIT_OBJECT_0,"MCP bridge process has already exited");
            bridges.push((pid,handle));
        }
        Ok(())
    }

    /// Stop the HTTP server first. A failure must prevent installing an update;
    /// this function neither kills processes nor discards live process handles.
    pub async fn wait_for_bridges(&self)->anyhow::Result<()> {
        self.wait_for_bridges_with_timeout(Duration::from_secs(10)).await
    }
    async fn wait_for_bridges_with_timeout(&self,timeout:Duration)->anyhow::Result<()> {
        #[cfg(windows)] {
            use windows_sys::Win32::{Foundation::{WAIT_OBJECT_0,WAIT_TIMEOUT},System::Threading::WaitForSingleObject};
            let deadline=tokio::time::Instant::now()+timeout;
            loop {
                {
                    let mut bridges=self.bridges.lock().unwrap_or_else(|error|error.into_inner());
                    let mut failed=false;
                    bridges.retain(|(_,handle)|match unsafe {WaitForSingleObject(handle.0,0)} {
                        WAIT_OBJECT_0=>false,WAIT_TIMEOUT=>true,_=>{failed=true;true}
                    });
                    anyhow::ensure!(!failed,"Unable to confirm MCP bridge shutdown; close the AI connection before updating");
                    if bridges.is_empty() {return Ok(());}
                }
                anyhow::ensure!(tokio::time::Instant::now()<deadline,"MCP bridge is still running; close its AI client connection and retry the update");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
        #[cfg(not(windows))] {let _=timeout;Ok(())}
    }
}

#[cfg(windows)]
struct ProcessHandle(windows_sys::Win32::Foundation::HANDLE);
// Native process handles can be waited on and closed from any thread. Access
// to this owner remains serialized by the registry mutex.
#[cfg(windows)] unsafe impl Send for ProcessHandle {}
#[cfg(windows)] impl Drop for ProcessHandle {fn drop(&mut self){unsafe {windows_sys::Win32::Foundation::CloseHandle(self.0);}}}
fn bounded(value:&str, max:usize)->String {
    let mut result=String::new();
    for ch in value.chars().filter(|ch|!ch.is_control()) {
        if result.len()+ch.len_utf8()>max {break;}
        result.push(ch);
    }
    result.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_expiring_activity_never_contains_client_extras() {
        let registry=ConnectionRegistry::default();
        let client=Implementation::new("AI\n客户端", "1");
        registry.record(&client,&ProtocolVersion::V_2026_07_28);
        registry.record(&client,&ProtocolVersion::V_2026_07_28);
        assert_eq!(registry.snapshot()[0].name,"AI客户端");
        assert_eq!(registry.snapshot()[0].requests,2);
        for index in 0..MAX_CLIENTS+8 {registry.record(&Implementation::new(format!("client-{index}"),"1"),&ProtocolVersion::V_2025_03_26);}
        assert_eq!(registry.snapshot().len(),MAX_CLIENTS);
        for entry in registry.entries.lock().unwrap().iter_mut() {entry.active=Instant::now()-TTL-Duration::from_secs(1);}
        assert!(registry.snapshot().is_empty());
        registry.record(&client,&ProtocolVersion::V_2026_07_28);
        registry.clear();assert!(registry.snapshot().is_empty());
    }

    #[test]
    fn bridge_process_fixture() {
        if std::env::var("HTTP_CAPTURE_BRIDGE_PROCESS_FIXTURE").as_deref()!=Ok("1") {return;}
        std::thread::sleep(Duration::from_millis(1800));
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn update_wait_verifies_process_image_preserves_live_handles_on_timeout_and_waits_for_exit() {
        use std::os::windows::process::CommandExt;
        let registry=ConnectionRegistry::default();
        assert!(registry.record_bridge_pid(std::process::id()).is_err());
        assert!(registry.record_bridge_pid(0).is_err());
        assert!(registry.record_bridge_pid(u32::MAX).is_err());
        let mut foreign=std::process::Command::new(std::env::var_os("COMSPEC").unwrap_or_else(||"cmd.exe".into()))
            .args(["/D","/Q","/C","set /p MCP_TEST="])
            .creation_flags(0x08000000).stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let unrelated=registry.record_bridge_pid(foreign.id());
        let _=foreign.kill();let _=foreign.wait();
        assert!(unrelated.is_err(),"an unrelated executable must never enter the updater wait list");
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","connections::tests::bridge_process_fixture","--nocapture"])
            .env("HTTP_CAPTURE_BRIDGE_PROCESS_FIXTURE","1")
            .creation_flags(0x08000000)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        registry.record_bridge_pid(child.id()).unwrap();
        registry.record_bridge_pid(child.id()).unwrap();
        assert_eq!(registry.bridges.lock().unwrap().len(),1,"repeated heartbeats must not duplicate process handles");
        registry.clear();
        assert!(registry.wait_for_bridges_with_timeout(Duration::from_millis(5)).await.is_err());
        assert_eq!(registry.bridges.lock().unwrap().len(),1,"timeout must retain the live OS process handle and block installation");
        registry.wait_for_bridges().await.unwrap();
        assert!(child.wait().unwrap().success());
        assert!(registry.bridges.lock().unwrap().is_empty());
    }
}
