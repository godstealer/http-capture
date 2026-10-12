//! Explicit system proxy ownership. Recovery never overwrites external changes.
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs::{self, File, OpenOptions}, io::Write, net::SocketAddr, path::PathBuf, sync::Mutex};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings { pub flags:u32, pub server:String, pub bypass:String, pub pac:String,
    #[serde(default, skip_serializing_if="Option::is_none")]
    pub gnome:Option<std::collections::BTreeMap<String,String>>,
}
#[derive(Serialize, Deserialize)]
struct Journal { version:u32, before:Settings, applied:Settings }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct Status { pub supported:bool, pub active:bool, pub pending_recovery:bool, pub message:String }
#[derive(Default)]
struct State { lease:Option<File>, active:bool, message:String }
pub struct SystemProxy { directory:PathBuf, state:Mutex<State> }
impl Default for SystemProxy {
    fn default()->Self {
        // Shared by desktop and development service, across their different data directories.
        let root=std::env::var_os("LOCALAPPDATA").or_else(||std::env::var_os("XDG_STATE_HOME"))
            .map(PathBuf::from).unwrap_or_else(||std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/state"));
        Self { directory:root.join("http-capture/system-proxy"), state:Mutex::new(State::default()) }
    }
}
impl SystemProxy {
    pub fn status(&self)->Status {
        let state=self.state.lock().unwrap();
        Status { supported:platform::supported(), active:state.active, pending_recovery:self.directory.join("recovery.json").exists(), message:state.message.clone() }
    }
    fn lease(&self,state:&mut State)->Result<()> {
        if state.lease.is_none() {
            fs::create_dir_all(&self.directory)?;
            #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;fs::set_permissions(&self.directory,fs::Permissions::from_mode(0o700))?;}
            let file=OpenOptions::new().read(true).write(true).create(true).truncate(false).open(self.directory.join("owner.lock"))?;
            file.try_lock().context("Another HTTP Capture process owns system proxy settings")?;
            state.lease=Some(file);
        }
        Ok(())
    }
    pub fn enable(&self, mut address:SocketAddr)->Result<()> {
        ensure!(platform::supported(),"System proxy requires Windows or a GNOME desktop session with gsettings and dconf");
        if address.ip().is_unspecified() { address.set_ip(if address.is_ipv4(){"127.0.0.1".parse()?}else{"::1".parse()?}); }
        ensure!(address.port()!=0,"Proxy listener is not running");
        let mut state=self.state.lock().unwrap(); self.lease(&mut state)?;
        ensure!(!self.directory.join("recovery.json").exists(),"Restore the previous system proxy configuration first");
        let before=platform::read()?;
        let applied=platform::target(&before,address)?;
        let journal=Journal{version:1,before,applied};
        // Durable intent is written BEFORE touching OS settings. A partial journal fails closed.
        let mut file=OpenOptions::new().write(true).create_new(true).open(self.directory.join("recovery.json"))?;
        file.write_all(&serde_json::to_vec(&journal)?)?;file.sync_all()?;
        let result=(|| {ensure!(platform::read()?==journal.before,"System proxy changed during configuration; no settings were written");platform::write(&journal.applied)})().and_then(|()| {ensure!(platform::read()?==journal.applied,"System proxy read-back differs from requested settings");Ok(())});
        state.active=result.is_ok();
        state.message=result.as_ref().err().map(ToString::to_string).unwrap_or_default();
        result
    }
    pub fn restore(&self)->Result<()> {
        let mut state=self.state.lock().unwrap();
        let result:Result<()>=(|| {
            if !self.directory.join("recovery.json").exists(){return Ok(());}
            self.lease(&mut state)?;
            recover(&self.directory.join("recovery.json"),platform::read,platform::write)?;
            Ok(())
        })();
        if result.is_ok(){state.active=false;state.lease=None;state.message.clear();}
        else {state.message=result.as_ref().err().unwrap().to_string();}
        result
    }
}
fn recover(path:&std::path::Path,read:impl Fn()->Result<Settings>,write:impl Fn(&Settings)->Result<()>)->Result<()> {
    let journal:Journal=serde_json::from_slice(&fs::read(path)?)?;
    ensure!(journal.version==1,"Unsupported system proxy recovery journal");
    if needs_restore(&journal,&read()?)? {write(&journal.before)?;ensure!(read()?==journal.before,"System proxy restoration could not be verified");}
    fs::remove_file(path)?;Ok(())
}
fn needs_restore(journal:&Journal,current:&Settings)->Result<bool> {
    if current==&journal.before {return Ok(false);}
    if current==&journal.applied {return Ok(true);}
    bail!("System proxy was changed by another application. Recovery snapshot retained; current settings were not overwritten.")
}
#[cfg(not(any(windows,target_os="linux")))]
mod platform {
    use super::*;
    pub fn supported()->bool{false}
    pub fn target(_: &Settings,_:SocketAddr)->Result<Settings>{bail!("Unsupported platform")}
    pub fn read()->Result<Settings>{bail!("Automatic system proxy management is not available on this platform")}
    pub fn write(_: &Settings)->Result<()>{bail!("Automatic system proxy management is not available on this platform")}
}
#[cfg(windows)]
#[path="system_proxy_windows.rs"] mod platform;
#[cfg(target_os="linux")]
#[path="system_proxy_gnome.rs"] mod platform;
#[cfg(all(test,not(target_os="linux")))]
#[allow(dead_code)]
#[path="system_proxy_gnome.rs"] mod gnome_tests;
#[cfg(test)] mod tests {
    use super::*;
    fn sample(flags:u32)->Settings{Settings{flags,server:"old:8080".into(),bypass:"*.internal".into(),pac:"https://local/proxy.pac".into(),gnome:None}}
    #[test] fn recovery_preserves_external_changes_and_pac(){
        let j=Journal{version:1,before:sample(13),applied:sample(3)};
        assert!(needs_restore(&j,&j.applied).unwrap());
        assert!(!needs_restore(&j,&j.before).unwrap());
        let mut external=j.applied.clone();external.pac="https://other/pac".into();
        assert!(needs_restore(&j,&external).is_err());
        let copy:Journal=serde_json::from_slice(&serde_json::to_vec(&j).unwrap()).unwrap();assert_eq!(copy.before,j.before);
    }
    #[test] fn ownership_is_exclusive_and_released(){
        let dir=tempfile::tempdir().unwrap();let p=SystemProxy{directory:dir.path().to_owned(),state:Mutex::new(State::default())};
        let mut a=State::default();let mut b=State::default();p.lease(&mut a).unwrap();assert!(p.lease(&mut b).is_err());drop(a);p.lease(&mut b).unwrap();
    }
    #[test] fn crash_recovery_restores_and_retains_failed_or_conflicting_journals(){
        use std::cell::RefCell;
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("recovery.json");
        let j=Journal{version:1,before:sample(13),applied:sample(3)};
        let current=RefCell::new(j.applied.clone());
        let save=||fs::write(&path,serde_json::to_vec(&j).unwrap()).unwrap();
        save();recover(&path,||Ok(current.borrow().clone()),|v|{*current.borrow_mut()=v.clone();Ok(())}).unwrap();
        assert_eq!(*current.borrow(),j.before);assert!(!path.exists());
        *current.borrow_mut()=j.applied.clone();save();
        assert!(recover(&path,||Ok(current.borrow().clone()),|_|bail!("permission denied")).is_err());assert!(path.exists());
        assert!(recover(&path,||Ok(current.borrow().clone()),|_|Ok(())).is_err());assert!(path.exists());
        current.borrow_mut().server="external:1234".into();
        assert!(recover(&path,||Ok(current.borrow().clone()),|_|panic!("must not change external settings")).is_err());assert!(path.exists());
        fs::write(&path,b"partial journal").unwrap();
        assert!(recover(&path,||panic!("must not read on corrupt journal"),|_|panic!()).is_err());assert!(path.exists());
    }
    #[cfg(windows)] #[test] fn windows_settings_read_only(){let settings=platform::read().unwrap();assert!(settings.flags>0);}

}
