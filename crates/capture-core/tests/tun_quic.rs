use capture_core::{Engine, model::*, transport::*};
use std::{sync::Arc,time::Duration};
use bytes::{Buf,Bytes};
use base64::{Engine as _,engine::general_purpose::STANDARD};

struct Echo;
impl EngineContract<RequestDraft,capture_core::upstream::UpstreamProxy> for Echo {
    fn id(&self)-> &'static str {"h3"}
    fn profiles(&self)->Vec<String>{vec!["native".into()]}
    fn send<'a>(&'a self,r:&'a RequestDraft)->SendFuture<'a>{Box::pin(async move {
        Ok((CapturedResponse{status:200,version:"HTTP/3".into(),headers:vec![],body_base64:r.body_base64.clone(),raw_head_base64:None,upstream_tls:None,tls_version:Some("TLS 1.3".into()),sent_request_headers:None},vec![]))
    })}
}

#[tokio::test]
async fn dynamic_sni_capture_records_multiple_origins_and_stops() {
    tokio::time::timeout(Duration::from_secs(10),async {
        let directory=tempfile::tempdir().unwrap();
        let mut transports=SendEngines::empty(); transports.register(Arc::new(Echo)).unwrap();
        let engine=Engine::open_with_engines(directory.path(),rustls::RootCertStore::empty(),transports).unwrap();
        let ingress=capture_core::quic_proxy::start_tun(engine.clone()).await.unwrap();
        let address=ingress.address;
        let pem=std::fs::read(&engine.ca.cert_path).unwrap();
        let mut roots=rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut &pem[..]) {roots.add(cert.unwrap()).unwrap();}
        for name in ["first.test","second.test"] {
            let tls=capture_core::tls_config::config(&TlsProfile::default(),&roots,"h3").unwrap();
            let mut client=quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
            client.set_default_client_config(quinn::ClientConfig::new(Arc::new(quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap())));
            let connection=client.connect(address,name).unwrap().await.unwrap();
            let (mut driver,mut sender)=h3::client::new(h3_quinn::Connection::new(connection.clone())).await.unwrap();
            let task=tokio::spawn(async move {let _=driver.wait_idle().await;});
            let mut stream=sender.send_request(http::Request::builder().method("POST").uri(format!("https://{name}/test?q=1")).body(()).unwrap()).await.unwrap();
            let content=vec![7;8192];stream.send_data(Bytes::from(content.clone())).await.unwrap();stream.finish().await.unwrap();
            assert_eq!(stream.recv_response().await.unwrap().status(),200);
            let mut received=vec![];while let Some(mut chunk)=stream.recv_data().await.unwrap(){received.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining()));}
            assert_eq!(received,content);
            let mut invalid=sender.send_request(http::Request::builder().uri("https://different.test/").body(()).unwrap()).await.unwrap();
            invalid.finish().await.unwrap();assert!(invalid.recv_response().await.is_err());
            connection.close(0u32.into(),b"done");task.abort();
        }
        let flows=engine.store.list().unwrap();assert_eq!(flows.len(),2);
        for flow in flows {
            assert_eq!(flow.client_protocol.as_deref(),Some("HTTP/3"));
            assert_eq!(flow.request.url,format!("https://{}/test?q=1",flow.client_tls.unwrap().server_name.unwrap()));
            assert_eq!(STANDARD.decode(flow.response.unwrap().body_base64).unwrap().len(),8192);
        }
        ingress.stop().await;
        // Quinn's endpoint driver releases the OS socket after its final wakeup.
        tokio::time::timeout(Duration::from_secs(2),async {
            loop { if let Ok(socket)=std::net::UdpSocket::bind(address) {break socket;} tokio::task::yield_now().await; }
        }).await.unwrap();
    }).await.unwrap();
}
