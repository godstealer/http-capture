//! Opt-in privileged test; selects only a dedicated copy of this executable.
use anyhow::{ensure,Result};
use capture_core::{Engine,model::RequestDraft,tun::TunConfig};
use std::time::Duration;

#[tokio::main]
async fn main()->Result<()> {
    let args:Vec<_>=std::env::args().collect();
    if args.get(1).is_some_and(|a|a=="--client") {
        std::env::set_var("HTTP_CAPTURE_QUIC_SOCKET_DEBUG","1");
        let pem=std::fs::read(&args[2])?;
        let mut roots=rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut &pem[..]) {roots.add(cert?)?;}
        let request:RequestDraft=serde_json::from_value(serde_json::json!({"url":"https://tls3.peet.ws/api/all","method":"GET","engine":"h3","headers":[],"bodyBase64":""}))?;
        let (response,_)=capture_core::replay::send_with_roots(&request,&roots).await?;
        ensure!(response.status==200 && response.version=="HTTP/3","H3 client response mismatch");
        println!("client=200/HTTP3");return Ok(());
    }
    // Diagnostics must exist before helper startup opens its log file.
    std::fs::create_dir_all(".local")?;
    // Fail before changing system routes when the test's required upstream is absent.
    tokio::time::timeout(Duration::from_secs(3),tokio::net::TcpStream::connect("127.0.0.1:7897"))
        .await.map_err(|_|anyhow::anyhow!("Test prerequisite: SOCKS5 upstream 127.0.0.1:7897 timed out"))?
        .map_err(|error|anyhow::anyhow!("Test prerequisite: start SOCKS5 upstream 127.0.0.1:7897 first: {error}"))?;
    let directory=tempfile::tempdir()?;
    std::env::set_var("HTTP_CAPTURE_TUN_DEBUG_LOG",std::env::current_dir()?.join(".local/tun-h3-helper.log"));
    let client=directory.path().join(if cfg!(windows){"capture-quic-test-client.exe"}else{"capture-quic-test-client"});
    let client_source=std::env::var_os("HTTP_CAPTURE_TUN_TEST_CLIENT")
        .map(std::path::PathBuf::from).unwrap_or(std::env::current_exe()?);
    std::fs::copy(client_source,&client)?;
    let engine=Engine::open(&directory.path().join("data"))?;
    engine.upstream.update(capture_core::upstream::UpstreamInput{enabled:true,url:"socks5://127.0.0.1:7897".into(),username:String::new(),password:None,auth_enabled:false})?;
    let result:Result<()>=async {
        engine.tun.start(engine.clone(),TunConfig{applications:vec![client.to_string_lossy().into_owned()],block_quic:false,capture_quic:true}).await?;
        let mut command=tokio::process::Command::new(&client);command.kill_on_drop(true);
        #[cfg(windows)] command.creation_flags(0x08000000);
        let output=tokio::time::timeout(Duration::from_secs(45),command.arg("--client").arg(&engine.ca.cert_path).output()).await??;
        std::fs::write(".local/tun-h3-client.log",&output.stderr)?;
        ensure!(output.status.success(),"Test client failed: {}",String::from_utf8_lossy(&output.stderr));
        let flows=engine.store.list()?;
        ensure!(flows.iter().any(|flow|flow.client_protocol.as_deref()==Some("HTTP/3") && flow.response.as_ref().is_some_and(|r|r.status==200) && flow.error.is_none()),"No successful captured H3 flow");
        Ok(())
    }.await;
    let cleanup=engine.tun.stop().await;
    let flows=engine.store.list()?;
    let diagnostics:Vec<_>=flows.iter().map(|flow|serde_json::json!({
        "clientProtocol":flow.client_protocol,
        "status":flow.response.as_ref().map(|response|response.status),
        "upstreamProtocol":flow.response.as_ref().map(|response|response.version.as_str()),
        "error":flow.error
    })).collect();
    std::fs::write(".local/tun-h3-smoke-flows.json",serde_json::to_vec_pretty(&diagnostics)?)?;
    std::fs::write(".local/tun-h3-smoke-result.txt",format!("execution={result:?}\ncleanup={cleanup:?}\ncapturedFlows={}\n",flows.len()))?;
    result?;cleanup?;
    println!("Selected app -> TUN -> H3 capture -> SOCKS5 UDP -> tls3.peet.ws: 200; TUN stopped");
    Ok(())
}
