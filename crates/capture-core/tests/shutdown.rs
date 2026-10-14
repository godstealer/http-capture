use capture_core::{Engine, proxy, executions::Executions};
use tokio::{net::{TcpListener,TcpStream}, io::AsyncReadExt, time::{timeout,Duration}};

#[tokio::test]
async fn stopping_proxy_closes_clients_and_releases_listener() {
 let dir=tempfile::tempdir().unwrap();
 let engine=Engine::open(dir.path()).unwrap();
 let proxy=proxy::start(engine,0).await.unwrap();
 let address=proxy.address;
 let mut client=TcpStream::connect(address).await.unwrap();
 timeout(Duration::from_secs(3),proxy.stop()).await.unwrap();
 let listener=TcpListener::bind(address).await.expect("listener must be reusable immediately");
 let mut byte=[0];
 let result=timeout(Duration::from_secs(3),client.read(&mut byte)).await.unwrap();
 assert!(matches!(result,Ok(0)|Err(_)),"old client must be closed");
 drop(listener);
}

#[tokio::test]
async fn shutdown_cancels_active_and_reserved_executions() {
 let registry=Executions::default();
 let mut active=registry.begin(registry.prepare().unwrap()).unwrap();
 let reserved=registry.prepare().unwrap();
 registry.cancel_all();
 timeout(Duration::from_secs(1),active.cancelled()).await.unwrap();
 let mut later=registry.begin(reserved).unwrap();
 timeout(Duration::from_secs(1),later.cancelled()).await.unwrap();
}
