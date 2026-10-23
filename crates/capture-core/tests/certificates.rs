use capture_core::{Engine,certificates::{Import,Change},model::RequestDraft};
use base64::{engine::general_purpose::STANDARD,Engine as _};
use std::sync::Arc;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};
fn configure(engine:&Engine,kind:&str,certificate:&str,key:&str,port:u16){
 let state=engine.certificates.import(Import{name:kind.into(),kind:kind.into(),host:"localhost".into(),port,certificate_base64:STANDARD.encode(certificate),private_key_pem:key.into(),container_format:None,password:None,expected_version:engine.certificates.snapshot().unwrap().version}).unwrap();
 engine.certificates.change(Change{action:"enable".into(),id:Some(state.entries.last().unwrap().id.clone()),enabled:true,expected_version:state.version}).unwrap();
}
#[tokio::test]
async fn native_h2_and_capture_authenticate_with_scoped_client_identity_and_keep_secrets_out_of_flows(){
 tokio::time::timeout(std::time::Duration::from_secs(30),async{
 let _=rustls::crypto::ring::default_provider().install_default();
 let mut p=rcgen::CertificateParams::new(vec![]).unwrap();p.distinguished_name.push(rcgen::DnType::CommonName,"Isolated certificate test CA");p.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);p.key_usages=vec![rcgen::KeyUsagePurpose::KeyCertSign];let ca_key=rcgen::KeyPair::generate().unwrap();let ca=p.self_signed(&ca_key).unwrap();
 let server_key=rcgen::KeyPair::generate().unwrap();let mut p=rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();p.extended_key_usages=vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];let server=p.signed_by(&server_key,&ca,&ca_key).unwrap();
 let client_key=rcgen::KeyPair::generate().unwrap();let mut p=rcgen::CertificateParams::new(vec![]).unwrap();p.extended_key_usages=vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];let client=p.signed_by(&client_key,&ca,&ca_key).unwrap();
 let mut roots=rustls::RootCertStore::empty();roots.add(ca.der().clone()).unwrap();let verifier=rustls::server::WebPkiClientVerifier::builder(Arc::new(roots)).build().unwrap();
 let mut config=rustls::ServerConfig::builder().with_client_cert_verifier(verifier).with_single_cert(vec![server.der().clone()],rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into()).unwrap();config.alpn_protocols=vec![b"h2".to_vec(),b"http/1.1".to_vec()];
 let mut h3_config=config.clone();h3_config.alpn_protocols=vec![b"h3".to_vec()];
 let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let acceptor=tokio_rustls::TlsAcceptor::from(Arc::new(config));let expected=client.der().clone();
 let server=tokio::spawn(async move{loop{let(tcp,_)=listener.accept().await.unwrap();let acceptor=acceptor.clone();let expected=expected.clone();tokio::spawn(async move{
  let Ok(mut tls)=acceptor.accept(tcp).await else{return};assert_eq!(tls.get_ref().1.peer_certificates().unwrap()[0],expected);
  if tls.get_ref().1.alpn_protocol()==Some(b"h2") {let mut connection=h2::server::handshake(tls).await.unwrap();while let Some(result)=connection.accept().await{let(_,mut response)=result.unwrap();let mut stream=response.send_response(http::Response::builder().status(200).body(()).unwrap(),false).unwrap();stream.send_data(bytes::Bytes::from_static(b"mtls-ok"),true).unwrap();}}
  else{let mut head=[0;4096];if tls.read(&mut head).await.is_ok(){let _=tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nmtls-ok").await;}}
 });}});
 let dir=tempfile::tempdir().unwrap();let engine=Engine::open(dir.path()).unwrap();configure(&engine,"trust",&ca.pem(),"",port);
 let draft=|engine:&str|serde_json::from_value::<RequestDraft>(serde_json::json!({"engine":engine,"method":"GET","url":format!("https://localhost:{port}/"),"headers":[],"bodyBase64":"","tls":{"preset":"native"}})).unwrap();
 let missing=engine.execute(draft("native"),None,"replay",None).await.unwrap();assert!(missing.error.is_some());
 configure(&engine,"client",&(client.pem()+&ca.pem()),&client_key.serialize_pem(),port);
 let mut engines=vec!["native","auto","h2"];
 if cfg!(feature="browser-replay"){engines.push("wreq")}
 for kind in engines{let mut request=draft(kind);if kind=="wreq"{request.tls.preset="chrome".into();request.tls.browser_version=Some("auto".into());}let flow=engine.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);assert_eq!(flow.response.as_ref().unwrap().status,200);assert_eq!(STANDARD.decode(&flow.response.as_ref().unwrap().body_base64).unwrap(),b"mtls-ok");let json=serde_json::to_string(&flow).unwrap();assert!(!json.contains("PRIVATE KEY"));assert!(!json.contains("clientPrivateKey"));}
 // QUIC/HTTP3 uses the same scoped identity, with mandatory client authentication.
 let endpoint=quinn::Endpoint::server(quinn::ServerConfig::with_crypto(Arc::new(quinn::crypto::rustls::QuicServerConfig::try_from(h3_config).unwrap())),"127.0.0.1:0".parse().unwrap()).unwrap();
 let h3_port=endpoint.local_addr().unwrap().port();
 let h3_server=tokio::spawn(async move{let connection=endpoint.accept().await.unwrap().await.unwrap();let mut server=h3::server::builder().build(h3_quinn::Connection::new(connection)).await.unwrap();let request=server.accept().await.unwrap().unwrap();let task=tokio::spawn(async move{let(_,mut stream)=request.resolve_request().await.unwrap();stream.send_response(http::Response::builder().status(200).body(()).unwrap()).await.unwrap();stream.send_data(bytes::Bytes::from_static(b"mtls-h3")).await.unwrap();stream.finish().await.unwrap();});let _=server.accept().await;task.await.unwrap();});
 configure(&engine,"trust",&ca.pem(),"",h3_port);configure(&engine,"client",&(client.pem()+&ca.pem()),&client_key.serialize_pem(),h3_port);
 let mut request=draft("h3");request.url=format!("https://localhost:{h3_port}/");let flow=engine.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);assert_eq!(flow.response.as_ref().unwrap().version,"HTTP/3");assert_eq!(STANDARD.decode(&flow.response.unwrap().body_base64).unwrap(),b"mtls-h3");h3_server.abort();let _=h3_server.await;
 // Eager upstream negotiation used by capture must have the same identity.
 let proxy=capture_core::proxy::start(engine.clone(),0).await.unwrap();let mut socket=tokio::net::TcpStream::connect(proxy.address).await.unwrap();socket.write_all(format!("CONNECT localhost:{port} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes()).await.unwrap();let mut response=Vec::new();while !response.ends_with(b"\r\n\r\n"){response.push(socket.read_u8().await.unwrap());}assert!(response.starts_with(b"HTTP/1.1 200"));
 let mut roots=rustls::RootCertStore::empty();for der in rustls_pemfile::certs(&mut std::fs::read_to_string(&engine.ca.cert_path).unwrap().as_bytes()){roots.add(der.unwrap()).unwrap();}
 let mut config=rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();config.alpn_protocols=vec![b"http/1.1".to_vec()];let mut tls=tokio_rustls::TlsConnector::from(Arc::new(config)).connect("localhost".try_into().unwrap(),socket).await.unwrap();tls.write_all(format!("GET / HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();let mut received=Vec::new();tls.read_to_end(&mut received).await.unwrap();assert!(received.windows(7).any(|v|v==b"mtls-ok"));proxy.stop().await;
 server.abort();let _=server.await;
 }).await.expect("Certificate integration timed out");
}
