use capture_core::tun::{helper_config,TunConfig};
use std::{io::Write,process::{Command,Stdio}};
#[test]
#[ignore = "requires scripts/build-tun.ps1; does not create TUN or modify routes"]
fn helper_accepts_generated_configuration() {
    let helper=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2).unwrap().join(".local/tun").join(if cfg!(windows){"http-capture-tun.exe"}else{"http-capture-tun"});
    for capture in [false,true] {
    let mut process=Command::new(&helper).arg("--check").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let config=if capture { capture_core::tun::helper_config_with_quic(&TunConfig{applications:vec!["curl.exe".into()],block_quic:false,capture_quic:true},8080,Some(9000)).unwrap() }
        else {helper_config(&TunConfig{applications:vec!["curl.exe".into()],block_quic:true,capture_quic:false},8080).unwrap()};
    writeln!(process.stdin.take().unwrap(),"{}",config).unwrap();
    let output=process.wait_with_output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stdout).contains("\"valid\":true"));
    }
}
