//! GNOME user-session proxy settings. dconf load applies one atomic changeset.
use super::*;
use std::{collections::BTreeMap,process::{Command,Stdio},io::Read,time::{Duration,Instant}};
const KEYS:&[&str]=&["mode","autoconfig-url","ignore-hosts","use-same-proxy","http/host","http/port","http/use-authentication","https/host","https/port","ftp/host","socks/host"];
pub fn supported()->bool {
    let desktop=std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
    desktop.split(':').any(|v|v=="gnome") && std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
        && ["/usr/bin/gsettings","/usr/bin/dconf"].iter().all(|p|std::path::Path::new(p).is_file())
        && std::env::var("GSETTINGS_BACKEND").map_or(true,|v|v=="dconf")
}
fn command(program:&str,args:&[&str],input:Option<&str>)->Result<String>{
    let mut child=Command::new(program).args(args).env("LC_ALL","C").stdin(if input.is_some(){Stdio::piped()}else{Stdio::null()}).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().with_context(||format!("Start {program}"))?;
    let out=child.stdout.take().unwrap();let err=child.stderr.take().unwrap();
    let reader=|stream:Box<dyn Read+Send>|std::thread::spawn(move||{let mut bytes=Vec::new();let result=stream.take(65536).read_to_end(&mut bytes);result.map(|_|bytes)});
    let stdout=reader(Box::new(out));let stderr=reader(Box::new(err));
    let outcome=(||{
        if let Some(input)=input {child.stdin.take().unwrap().write_all(input.as_bytes())?;}
        let started=Instant::now();loop {
            if let Some(status)=child.try_wait()? {return Ok(status);}
            ensure!(started.elapsed()<Duration::from_secs(8),"{program} timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    if outcome.is_err(){let _=child.kill();let _=child.wait();}
    let stdout=stdout.join().map_err(|_|anyhow::anyhow!("Output reader failed"))??;
    let stderr=stderr.join().map_err(|_|anyhow::anyhow!("Error reader failed"))??;
    ensure!(outcome?.success(),"{program}: {}",String::from_utf8_lossy(&stderr));
    Ok(String::from_utf8(stdout)?.trim().to_owned())
}
fn schema(key:&str)->(String,&str){match key.split_once('/') {Some((group,key))=>(format!("org.gnome.system.proxy.{group}"),key),None=>("org.gnome.system.proxy".into(),key)}}
pub fn read()->Result<Settings>{
    ensure!(supported(),"GNOME session, dconf and gsettings are required");
    let mut values=BTreeMap::new();
    for key in KEYS {let (schema,name)=schema(key);let value=command("/usr/bin/gsettings",&["get",&schema,name],None)?;ensure!(!value.is_empty(),"Empty GNOME setting: {key}");values.insert((*key).into(),value);}
    Ok(Settings{flags:0,server:String::new(),bypass:String::new(),pac:String::new(),gnome:Some(values)})
}
pub fn target(before:&Settings,address:SocketAddr)->Result<Settings>{
    let mut result=before.clone();let values=result.gnome.as_mut().context("Missing GNOME snapshot")?;
    // Only literal IPs enter the GVariant string; no shell interpolation.
    for (key,value) in [("mode","'manual'".into()),("use-same-proxy","false".into()),("http/use-authentication","false".into()),
        ("http/host",format!("'{}'",address.ip())),("https/host",format!("'{}'",address.ip())),("http/port",address.port().to_string()),("https/port",address.port().to_string()),
        ("ftp/host","''".into()),("socks/host","''".into())] {values.insert(key.into(),value);}
    // Preserve PAC URL and bypass rules without activating PAC or unrelated protocols.
    Ok(result)
}
fn keyfile(settings:&Settings)->Result<String>{
    let values=settings.gnome.as_ref().context("Not a GNOME recovery snapshot")?;
    ensure!(values.len()==KEYS.len()&&KEYS.iter().all(|k|values.contains_key(*k)),"Invalid GNOME snapshot keys");
    let mut groups:BTreeMap<&str,Vec<(&str,&str)>>=BTreeMap::new();
    for (path,value) in values {
        ensure!(!value.is_empty()&&!value.contains(['\r','\n','\0']),"Invalid GNOME setting value");
        let (group,key)=path.split_once('/').unwrap_or(("/",path));groups.entry(group).or_default().push((key,value));
    }
    let mut text=String::new();for (group,rows) in groups {text.push_str(&format!("[{group}]\n"));for (key,value) in rows {text.push_str(&format!("{key}={value}\n"));}text.push('\n');}Ok(text)
}
pub fn write(settings:&Settings)->Result<()>{
    ensure!(supported(),"GNOME session, dconf and gsettings are required");
    let payload=keyfile(settings)?;
    for key in KEYS {let(schema,name)=schema(key);ensure!(command("/usr/bin/gsettings",&["writable",&schema,name],None)?=="true","GNOME proxy setting is locked: {key}");}
    command("/usr/bin/dconf",&["load","/system/proxy/"],Some(&payload))?;Ok(())
}
#[cfg(test)]mod tests{
    use super::*;
    fn fixture()->Settings {Settings{flags:0,server:String::new(),pac:String::new(),bypass:String::new(),gnome:Some(KEYS.iter().map(|k|((*k).into(),"'original'".into())).collect())}}
    #[test]fn gnome_plan_preserves_pac_and_bypass_and_disables_old_auth(){let before=fixture();let after=target(&before,"[::1]:8080".parse().unwrap()).unwrap();let v=after.gnome.as_ref().unwrap();assert_eq!(v["http/host"],"'::1'");assert_eq!(v["https/port"],"8080");assert_eq!(v["mode"],"'manual'");assert_eq!(v["http/use-authentication"],"false");assert_eq!(v["autoconfig-url"],"'original'");assert_eq!(v["ignore-hosts"],"'original'");assert_eq!(v["socks/host"],"''");assert_ne!(before,after);let text=keyfile(&after).unwrap();assert!(text.contains("[/]\n"));assert!(text.contains("[https]\nhost='::1'\nport=8080"));}
    #[test]fn gnome_journal_rejects_key_and_newline_injection(){let mut s=fixture();s.gnome.as_mut().unwrap().insert("mode".into(),"'none'\n[other]\nx=1".into());assert!(keyfile(&s).is_err());let mut s=fixture();s.gnome.as_mut().unwrap().insert("../../other".into(),"true".into());assert!(keyfile(&s).is_err());}
    #[test]fn legacy_windows_journal_still_loads(){let s:Settings=serde_json::from_str(r#"{"flags":3,"server":"old","bypass":"local","pac":""}"#).unwrap();assert!(s.gnome.is_none());assert!(keyfile(&s).is_err());}
    #[cfg(target_os="linux")]
    #[test]
    #[ignore = "requires a private dbus-run-session and disposable XDG_CONFIG_HOME; see scripts/test-gnome-proxy.sh"]
    fn gnome_isolated_session_roundtrip(){
        assert_eq!(std::env::var("HTTP_CAPTURE_GNOME_TEST").as_deref(),Ok("1"));
        let config=PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").expect("isolated config required"));
        assert!(config.file_name().unwrap().to_string_lossy().starts_with("http-capture-gnome-"));
        let dir=tempfile::tempdir().unwrap();
        let manager=||SystemProxy{directory:dir.path().to_owned(),state:Mutex::new(State::default())};
        let original=read().unwrap();
        let first=manager();first.enable("127.0.0.1:18987".parse().unwrap()).unwrap();
        assert!(first.status().active);let applied=read().unwrap();assert_ne!(applied,original);
        drop(first); // Simulate termination without running exit restoration.
        let restarted=manager();restarted.restore().unwrap();assert_eq!(read().unwrap(),original);
        restarted.enable("[::1]:18987".parse().unwrap()).unwrap();let applied=read().unwrap();
        command("/usr/bin/gsettings",&["set","org.gnome.system.proxy","mode","'none'"],None).unwrap();
        assert!(restarted.restore().is_err());assert!(restarted.status().pending_recovery);
        assert_eq!(read().unwrap().gnome.unwrap()["mode"],"'none'");
        write(&applied).unwrap();restarted.restore().unwrap();assert_eq!(read().unwrap(),original);
    }

}
