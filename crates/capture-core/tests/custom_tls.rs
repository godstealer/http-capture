use capture_core::{ca::CertificateAuthority, model::{RequestDraft,TlsProfile}, replay::send_with_roots};
use std::sync::Arc;
use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpListener};
use tokio_rustls::TlsAcceptor;

#[tokio::test]
async fn custom_tls_versions_and_ciphers_are_negotiated() {
 let _=rustls::crypto::ring::default_provider().install_default();
 let dir=tempfile::tempdir().unwrap();
 let ca=CertificateAuthority::load_or_create(dir.path()).unwrap();
 let mut roots=rustls::RootCertStore::empty();
 for cert in rustls_pemfile::certs(&mut std::io::BufReader::new(std::fs::File::open(&ca.cert_path).unwrap())) { roots.add(cert.unwrap()).unwrap(); }
 for (version,cipher) in [("1.3","TLS_AES_128_GCM_SHA256"),("1.2","TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256")] {
  let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
  let config=ca.server_config("localhost").unwrap();
  let expected=version.to_string();let expected_cipher=cipher.to_string();
  let server=tokio::spawn(async move {
   let (tcp,_)=listener.accept().await.unwrap();let mut tls=TlsAcceptor::from(config).accept(tcp).await.unwrap();
   let connection=tls.get_ref().1;
   assert_eq!(format!("{:?}",connection.protocol_version().unwrap()),if expected=="1.3" {"TLSv1_3"} else {"TLSv1_2"});
   assert_eq!(format!("{:?}",connection.negotiated_cipher_suite().unwrap().suite()).replace("TLS13_","TLS_"),expected_cipher);
   let mut bytes=[0;4096];tls.read(&mut bytes).await.unwrap();
   tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").await.unwrap();
  });
  let draft:RequestDraft=serde_json::from_value(serde_json::json!({"engine":"native","method":"GET","url":format!("https://localhost:{}/",addr.port()),"headers":[],"bodyBase64":"","tls":{"preset":"native","version":version,"cipherList":cipher,"curvesList":"secp256r1:X25519"}})).unwrap();
  let (response,_)=send_with_roots(&draft,&roots).await.unwrap();assert_eq!(response.status,200);server.await.unwrap();
 }
 // Customizing TLS must not weaken trust validation.
 let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
 let acceptor=TlsAcceptor::from(Arc::clone(&ca.server_config("localhost").unwrap()));
 let server=tokio::spawn(async move {let (tcp,_)=listener.accept().await.unwrap();assert!(acceptor.accept(tcp).await.is_err());});
 let draft:RequestDraft=serde_json::from_value(serde_json::json!({"engine":"auto","method":"GET","url":format!("https://localhost:{}/",addr.port()),"headers":[],"bodyBase64":"","tls":TlsProfile{version:Some("1.3".into()),..Default::default()}})).unwrap();
 assert!(send_with_roots(&draft,&rustls::RootCertStore::empty()).await.is_err());server.await.unwrap();
}
