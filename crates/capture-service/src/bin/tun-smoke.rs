//! Opt-in privileged smoke test. Never run automatically with the normal test suite.
use anyhow::{ensure,Result};
use capture_core::{Engine,tun::TunConfig};
use std::{path::PathBuf,time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().to_path_buf();
    let data=root.join(".local/tun-smoke").join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&data)?;
    let original=if cfg!(windows){PathBuf::from("C:/Windows/System32/curl.exe")}else{PathBuf::from("/usr/bin/curl")};
    let client=data.join(if cfg!(windows){"capture-tun-test-client.exe"}else{"capture-tun-test-client"});
    std::fs::copy(&original,&client)?;
    let engine=Engine::open(&data)?;
    let config=TunConfig{applications:vec![client.to_string_lossy().into_owned()],block_quic:true,capture_quic:false};
    let report=root.join(".local/tun-smoke-result.txt");
    let outcome:Result<()>=async {
        engine.tun.start(engine.clone(),config).await?;
        if std::env::args().any(|a|a=="--parent-exit") {
            // Deliberately bypass Rust destructors: the OS closes the pipe handle.
            std::fs::write(root.join(".local/tun-parent-exit.txt"),"started=true\n")?;
            std::process::exit(0);
        }
        for (program,scheme,expect_capture) in [(&original,"https",false),(&client,"http",true),(&client,"https",true)] {
            let before=engine.store.list()?.len();
            let mut command=tokio::process::Command::new(program);
            command.kill_on_drop(true);
            #[cfg(windows)] command.creation_flags(0x08000000);
            if cfg!(windows) { command.arg("--ssl-revoke-best-effort"); }
            if expect_capture { command.arg("--cacert").arg(&engine.ca.cert_path); }
            let output=tokio::time::timeout(Duration::from_secs(30),command.args(["--noproxy","*","--ipv4","--silent","--show-error","--max-time","20"])
                .args(["--output",if cfg!(windows){"NUL"}else{"/dev/null"},"--write-out","%{http_code}"])
                .arg(format!("{scheme}://httpbin.org/get?capture_tun_smoke=1")).output()).await??;
            ensure!(output.status.success(),"{scheme} curl failed (selected={expect_capture}): {}",String::from_utf8_lossy(&output.stderr));
            let flows=engine.store.list()?;
            if expect_capture {
                ensure!(flows.len()>before,"selected application was not captured");
                ensure!(flows.iter().any(|f|f.request.url.starts_with(scheme)&&f.response.is_some()&&f.error.is_none()),"missing successful captured response");
            } else {ensure!(flows.len()==before,"non-selected application was captured");}
        }
        Ok(())
    }.await;
    let stop=engine.tun.stop().await;
    let success=outcome.is_ok()&&stop.is_ok();
    std::fs::write(&report,format!("success={success}\nexecution={outcome:?}\ncleanup={stop:?}\n"))?;
    outcome?;stop?;println!("Selected app HTTP/HTTPS, non-selected bypass and TUN stop passed.");Ok(())
}
