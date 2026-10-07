//! Optional per-application TUN helper. Its stdin pipe is a lifetime lease.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, process::Stdio, sync::Arc, time::Duration};
use tokio::{io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader}, process::{Child, ChildStdin, Command}, sync::Mutex};
use crate::{Engine, proxy::ProxyHandle};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all="camelCase", deny_unknown_fields)]
pub struct TunConfig { pub applications: Vec<String>, #[serde(default)] pub block_quic: bool, #[serde(default)] pub capture_quic: bool }
impl TunConfig {
    pub fn validate(&self)->Result<()> {
        ensure!(!(self.block_quic && self.capture_quic),"H3 抓包和阻止 QUIC 不能同时启用");
        ensure!(!self.applications.is_empty() && self.applications.len()<=32,"请选择 1–32 个应用名称或可执行文件绝对路径");
        for app in &self.applications { ensure!(!app.trim().is_empty() && app.len()<=1024 && !app.chars().any(char::is_control),"应用名称或路径无效"); }
        Ok(())
    }
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct TunStatus { pub available: bool, pub running: bool, pub helper_path: String, pub config: Option<TunConfig>, pub error: Option<String> }
struct Running { child: Child, lease: Option<ChildStdin>, bridge: ProxyHandle, quic: Option<crate::quic_proxy::QuicHandle>, config: TunConfig, stderr: tokio::task::JoinHandle<String> }
#[derive(Default)]
struct Inner { running: Option<Running>, error: Option<String> }
#[derive(Default)]
pub struct TunState { inner: Mutex<Inner> }
fn helper_path()->PathBuf {
    let name=if cfg!(windows){"http-capture-tun.exe"}else{"http-capture-tun"};
    let alongside=std::env::current_exe().unwrap_or_default().with_file_name(name);
    if alongside.is_file(){return alongside;}
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().join(".local/tun").join(name)
}
fn helper_command()->Command {
    let mut c=Command::new(helper_path());
    #[cfg(windows)] c.creation_flags(0x08000000);
    c
}
fn matches_apps(apps:&[String])->Value {
    let (paths,names):(Vec<_>,Vec<_>)=apps.iter().cloned().partition(|s|s.contains('/')||s.contains('\\'));
    let mut rules=vec![];
    if !paths.is_empty(){
        if cfg!(windows) {
            let patterns:Vec<String>=paths.iter().map(|p|format!("(?i)^{}$",regex::escape(&p.replace('/',"\\")))).collect();
            rules.push(json!({"process_path_regex":patterns}));
        } else { rules.push(json!({"process_path":paths})); }
    }
    if !names.is_empty(){rules.push(json!({"process_name":names}));}
    json!({"type":"logical","mode":"or","rules":rules})
}
pub fn helper_config(config:&TunConfig,port:u16)->Result<Value> {
    helper_config_with_quic(config,port,None)
}
pub fn helper_config_with_quic(config:&TunConfig,port:u16,quic_port:Option<u16>)->Result<Value> {
    config.validate()?;
    ensure!(!config.capture_quic || quic_port.is_some_and(|p|p>0),"缺少 H3 抓包入口端口");
    let own=std::env::current_exe()?.to_string_lossy().into_owned();
    let helper=helper_path().to_string_lossy().into_owned();
    let selected=matches_apps(&config.applications);
    let own_name=std::env::current_exe()?.file_name().unwrap().to_string_lossy().into_owned();
    let helper_name=helper_path().file_name().unwrap().to_string_lossy().into_owned();
    let mut rules=vec![json!({"type":"logical","mode":"or","rules":[{"process_path":[own,helper]},{"process_name":[own_name,helper_name]}],"action":"route","outbound":"direct"})];
    if config.block_quic { rules.push(json!({"type":"logical","mode":"and","rules":[selected.clone(),{"network":"udp","port":443}],"action":"reject"})); }
    if config.capture_quic {
        rules.push(json!({"type":"logical","mode":"and","rules":[selected.clone(),{"network":"udp","port":443}],"action":"sniff","sniffer":["quic"],"timeout":"500ms"}));
        rules.push(json!({"type":"logical","mode":"and","rules":[selected.clone(),{"network":"udp","port":443,"protocol":"quic"}],"action":"route","outbound":"direct","override_address":"127.0.0.1","override_port":quic_port.unwrap()}));
    }
    rules.push(json!({"type":"logical","mode":"and","rules":[selected.clone(),{"network":"tcp"}],"action":"sniff","sniffer":["http","tls"],"timeout":"500ms"}));
    rules.push(json!({"type":"logical","mode":"and","rules":[selected,{"network":"tcp","protocol":["http","tls"]}],"action":"route","outbound":"capture"}));
    Ok(json!({
        "log":match std::env::var("HTTP_CAPTURE_TUN_DEBUG_LOG") { Ok(path) if !path.is_empty()=>json!({"level":"trace","output":path}), _=>json!({"disabled":true}) },
        "dns":{"servers":[{"type":"local","tag":"local"}],"final":"local"},
        "inbounds":[{"type":"tun","tag":"capture-tun","address":["172.31.255.1/30","fdfe:dcba:9876::1/126"],"mtu":1500,"auto_route":true,"strict_route":false,"dns_mode":"disabled","route_exclude_address":["127.0.0.0/8","::1/128"]}],
        "outbounds":[{"type":"direct","tag":"direct"},{"type":"http","tag":"capture","server":"127.0.0.1","server_port":port}],
        "route":{"auto_detect_interface":true,"find_process":true,"rules":rules,"final":"direct","default_domain_resolver":"local"}
    }))
}
impl TunState {
    pub async fn status(&self)->TunStatus {
        let mut inner=self.inner.lock().await;
        if inner.running.as_mut().is_some_and(|r|r.child.try_wait().ok().flatten().is_some()) {
            let r=inner.running.take().unwrap();
            let log=r.stderr.await.unwrap_or_default();r.bridge.stop().await;
            if let Some(quic)=r.quic { quic.stop().await; }
            inner.error=Some(if log.is_empty(){"TUN 辅助进程已退出".into()}else{log});
        }
        TunStatus{available:helper_path().is_file(),running:inner.running.is_some(),helper_path:helper_path().to_string_lossy().into_owned(),config:inner.running.as_ref().map(|r|r.config.clone()),error:inner.error.clone()}
    }
    pub async fn start(&self,engine:Arc<Engine>,config:TunConfig)->Result<()> {
        config.validate()?;
        let mut inner=self.inner.lock().await;
        ensure!(inner.running.is_none(),"TUN 已在运行，请先停止");
        ensure!(helper_path().is_file(),"尚未构建 TUN 辅助进程，请运行 scripts/build-tun.ps1");
        let quic=if config.capture_quic { Some(crate::quic_proxy::start_tun(engine.clone()).await?) } else { None };
        let bridge=crate::proxy::start_tun_bridge(engine).await?;
        let data=helper_config_with_quic(&config,bridge.address.port(),quic.as_ref().map(|q|q.address.port()))?;
        let mut child=helper_command().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("无法启动 TUN 辅助进程")?;
        let stderr=child.stderr.take().unwrap();
        let stderr=tokio::spawn(async move {let mut text=String::new();let _=stderr.take(8192).read_to_string(&mut text).await;text});
        let mut lease=child.stdin.take().unwrap();
        let mut stdout=BufReader::new(child.stdout.take().unwrap());
        let startup:Result<()>=async {
            lease.write_all(serde_json::to_string(&data)?.as_bytes()).await?;lease.write_all(b"\n").await?;lease.flush().await?;
            let mut line=String::new();
            tokio::time::timeout(Duration::from_secs(30),stdout.read_line(&mut line)).await.context("TUN 启动超时")??;
            ensure!(serde_json::from_str::<Value>(&line).ok().and_then(|v|v["ready"].as_bool())==Some(true),"TUN 未启动；请检查管理员/root 权限和辅助进程依赖");
            Ok(())
        }.await;
        if let Err(error)=startup {
            drop(lease);
            if tokio::time::timeout(Duration::from_secs(10),child.wait()).await.is_err() {
                inner.running=Some(Running{child,lease:None,bridge,quic,config,stderr});
                inner.error=Some(format!("{error:#}；仍在等待路由清理，请点击停止重试"));
                anyhow::bail!("{}",inner.error.as_ref().unwrap());
            }
            let log=stderr.await.unwrap_or_default();bridge.stop().await;
            if let Some(quic)=quic { quic.stop().await; }
            inner.error=Some(format!("{error:#} {log}"));anyhow::bail!("{}",inner.error.as_ref().unwrap());
        }
        inner.error=None;inner.running=Some(Running{child,lease:Some(lease),bridge,quic,config,stderr});Ok(())
    }
    pub async fn stop(&self)->Result<()> {
        let mut inner=self.inner.lock().await;
        if let Some(mut running)=inner.running.take() {
            running.lease.take();
            match tokio::time::timeout(Duration::from_secs(10),running.child.wait()).await {
                Ok(Ok(_))=>{running.bridge.stop().await;if let Some(quic)=running.quic {quic.stop().await;}let _=running.stderr.await;inner.error=None;}
                _=>{inner.running=Some(running);anyhow::bail!("辅助进程仍在退出，请稍后重试；未强制中断路由清理");}
            }
        }
        Ok(())
    }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn quic_redirect_is_opt_in_scoped_and_requires_listener() {
        let mut config=TunConfig{applications:vec!["browser.exe".into()],block_quic:false,capture_quic:true};
        assert!(helper_config(&config,8080).is_err());
        let v=helper_config_with_quic(&config,8080,Some(9000)).unwrap();
        let rules=v["route"]["rules"].as_array().unwrap();
        assert_eq!(rules[0]["outbound"],"direct");
        assert_eq!(rules[1]["sniffer"][0],"quic");
        assert_eq!(rules[2]["override_address"],"127.0.0.1");
        assert_eq!(rules[2]["override_port"],9000);
        assert_eq!(rules[2]["rules"][1]["port"],443);
        assert_eq!(rules[2]["rules"][1]["protocol"],"quic");
        assert_eq!(rules[2]["rules"][0],matches_apps(&config.applications));
        config.block_quic=true; assert!(config.validate().is_err());
    }
    #[test] fn selected_apps_and_loop_prevention(){
        assert!(helper_config(&TunConfig{applications:vec![],block_quic:false,capture_quic:false},8080).is_err());
        let v=helper_config(&TunConfig{applications:vec!["curl.exe".into(),"/usr/bin/curl".into()],block_quic:true,capture_quic:false},8080).unwrap();
        assert_eq!(v["route"]["final"],"direct");assert_eq!(v["route"]["rules"][0]["outbound"],"direct");
        assert_eq!(v["inbounds"][0]["dns_mode"],"disabled");
        assert_eq!(v["route"]["rules"][3]["outbound"],"capture");
        assert!(v["route"]["rules"][3]["rules"][0]["rules"].as_array().unwrap().len()==2);
    }
}
