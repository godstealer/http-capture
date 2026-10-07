use capture_core::{Engine, model::*, scripts::Scripts, transport::*, intercept::{Config,Rule}};
use std::{sync::{Arc,Mutex},time::{Instant,Duration}};
use base64::{Engine as _,engine::general_purpose::STANDARD};
struct Probe(Arc<Mutex<Vec<RequestDraft>>>);
impl EngineContract<RequestDraft,capture_core::upstream::UpstreamProxy> for Probe {
 fn id(&self)->&'static str{"probe"} fn profiles(&self)->Vec<String>{vec!["native".into()]}
 fn send<'a>(&'a self,r:&'a RequestDraft)->SendFuture<'a>{Box::pin(async move{self.0.lock().unwrap().push(r.clone());Ok((CapturedResponse{status:200,version:"HTTP/2".into(),headers:vec![Header{name:"Content-Encoding".into(),value:"br".into()}],body_base64:STANDARD.encode(b"original"),raw_head_base64:None,upstream_tls:None,tls_version:None,sent_request_headers:Some(r.headers.clone())},vec![]))})}
}
fn draft()->RequestDraft{serde_json::from_str(r#"{"method":"GET","url":"http://example.test/","engine":"probe","headers":[{"name":"X-A","value":"1"},{"name":"X-B","value":"2"},{"name":"X-A","value":"3"}],"bodyBase64":""}"#).unwrap()}
fn engine()->(tempfile::TempDir,Arc<Engine>,Arc<Mutex<Vec<RequestDraft>>>) {
 let dir=tempfile::tempdir().unwrap();let calls=Arc::new(Mutex::new(vec![]));let mut engines=SendEngines::empty();engines.register(Arc::new(Probe(calls.clone()))).unwrap();let engine=Engine::open_with_engines(dir.path(),rustls::RootCertStore::empty(),engines).unwrap();(dir,engine,calls)
}
#[tokio::test]
async fn script_pipeline_modifies_ordered_requests_responses_and_preserves_originals(){
 let (_dir,e,calls)=engine();let mut request=draft();request.scripts=Scripts{enabled:true,before:r#"
assert(typeof fetch === 'undefined' && typeof process === 'undefined' && typeof require === 'undefined');
request.method='POST';request.bodyBase64=encodeText('你好');
request.headers.push({name:'X-Sign',value:sha256('abc')});variables.token='abc';console.log('before');
"#.into(),after:r#"
assert(response.status===200,'status');assert(variables.token==='abc');response.status=201;
response.bodyBase64=encodeText('修改后');variables.token='updated';console.log('after');
"#.into(),..Default::default()};
 let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);
 assert_eq!(flow.original_request.unwrap().method,"GET");assert_eq!(flow.original_response.unwrap().status,200);
 let sent=&calls.lock().unwrap()[0];assert_eq!(sent.method,"POST");assert_eq!(sent.headers.iter().map(|h|h.name.as_str()).collect::<Vec<_>>(),["X-A","X-B","X-A","X-Sign"]);
 assert_eq!(sent.headers[3].value,"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
 let response=flow.response.unwrap();assert_eq!(response.status,201);assert_eq!(response.version,"HTTP/2");assert_eq!(STANDARD.decode(response.body_base64).unwrap(),"修改后".as_bytes());assert!(!response.headers.iter().any(|h|h.name.eq_ignore_ascii_case("content-encoding")));assert_eq!(flow.request.scripts.variables["token"],"updated");assert!(flow.notes.iter().any(|n|n.contains("after")));
}
#[tokio::test]
async fn capture_scripts_use_rules_without_manual_breakpoints_and_share_variables(){
 let (_dir,e,calls)=engine();e.intercept.configure(Config{request:false,response:false,scope:"capture".into(),rules:vec![Rule{host:"example.test".into(),..Default::default()}]}).unwrap();
 e.scripts.configure(Scripts{enabled:true,before:"variables.count=String(Number(variables.count||'0')+1);request.headers.push({name:'X-Count',value:variables.count});".into(),..Default::default()}).unwrap();
 for _ in 0..2{assert!(e.execute(draft(),None,"capture",None).await.unwrap().error.is_none());}
 let mut other=draft();other.url="http://other.test/".into();assert!(e.execute(other,None,"capture",None).await.unwrap().error.is_none());
 assert_eq!(e.scripts.snapshot().0.variables["count"],"2");assert_eq!(calls.lock().unwrap()[1].headers.last().unwrap().value,"2");assert_eq!(calls.lock().unwrap()[2].headers.len(),3);
}
#[tokio::test]
async fn failures_are_transactional_and_never_send_invalid_requests(){
 let (_dir,e,calls)=engine();
 for code in ["variables.x='not committed';console.log('before failure');throw new Error('boom');", "request.headers.push({name:'Bad\\r\\nName',value:'x'});", "request.bodyBase64='%%%';", "const = broken;", "return Promise.resolve();", "Promise.resolve().then(()=>request.method='POST');"] {
  let mut request=draft();request.scripts=Scripts{enabled:true,before:code.into(),..Default::default()};let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_some(),"{code}");assert!(!flow.request.scripts.variables.contains_key("x"));
 }
 assert!(calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn infinite_scripts_and_memory_growth_are_bounded(){
 let (_dir,e,calls)=engine();
 for code in ["while(true) {}", "const memory=[];while(true)memory.push(new Array(10000).fill(Math.random()));"] {
 let mut request=draft();request.scripts=Scripts{enabled:true,before:code.into(),..Default::default()};let start=Instant::now();let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_some());assert!(start.elapsed()<Duration::from_secs(5)); }
 assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn signing_helpers_match_known_vectors_and_utf8() {
 let (_dir,e,_calls)=engine();let mut request=draft();request.scripts=Scripts{enabled:true,before:r#"
assert(md5('abc')==='900150983cd24fb0d6963f7d28e17f72');
assert(md5('')==='d41d8cd98f00b204e9800998ecf8427e');
assert(md5('你好')==='7eca689f0d3389d9dea66ae112e5cfd7');
assert(sha256('')==='e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
assert(hmacSha256('key','The quick brown fox jumps over the lazy dog')==='f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8');
assert(hmacMd5('key','The quick brown fox jumps over the lazy dog')==='80070713463e7749b90c2dc24911e275');
const timestamp='1700000000';variables.signature=sha256(timestamp+'secret');
console.log(md5(timestamp));
"#.into(),..Default::default()};
 let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);assert_eq!(flow.request.scripts.variables["signature"].len(),64);
}

#[tokio::test]
async fn bundled_modules_preserve_bytes_and_support_aes_gcm_vectors(){
 let (_dir,e,_calls)=engine();let mut request=draft();request.scripts=Scripts{enabled:true,before:r#"
assert(encoding.base64Decode(encoding.base64Encode('你好'))==='你好');
assert(encoding.base64urlEncode('ffff','hex')==='__8');
assert(encoding.base64urlDecode('__8=','hex')==='ffff');
assert(encoding.hexDecode(encoding.hexEncode('你好'))==='你好');
assert(encoding.convert('00ff80',{from:'hex',to:'base64'})==='AP+A');
assert(crypto.hash('SHA-1','abc')==='a9993e364706816aba3e25717850c26c9cd0d89d');
assert(crypto.hash('SHA-512','abc').length===128);
assert(crypto.hmac('SHA-256',{key:'key',message:'The quick brown fox jumps over the lazy dog',output:'base64'})===encoding.convert('f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8',{from:'hex',to:'base64'}));
const fixed={mode:'AES-GCM',key:'00'.repeat(16),keyEncoding:'hex',nonce:'00'.repeat(12),nonceEncoding:'hex',inputEncoding:'hex',output:'hex'};
const ciphertext=crypto.encrypt({...fixed,data:'00'.repeat(16)});
assert(ciphertext==='0388dace60b6a392f328c2b971b2fe78ab6e47d42cec13bdf53a67b21257bddf');
assert(crypto.decrypt({...fixed,data:ciphertext})==='00'.repeat(16));
const options={mode:'AES-GCM',key:crypto.randomBytes(32),keyEncoding:'hex',nonce:crypto.randomBytes(12),nonceEncoding:'hex',aad:'context'};
const encrypted=crypto.encrypt({...options,data:'你好',inputEncoding:'utf8',output:'base64'});
assert(crypto.decrypt({...options,data:encrypted,inputEncoding:'base64',output:'utf8'})==='你好');
let rejected=false;try{crypto.decrypt({...options,aad:'wrong',data:encrypted,inputEncoding:'base64',output:'utf8'});}catch{rejected=true;}assert(rejected);
assert(crypto.randomBytes(16,'base64').length===24);
assert(/^[0-9a-f-]{36}$/.test(utils.uuid()));
assert(Math.abs(utils.timestamp('milliseconds')-Date.now())<5000);
"#.into(),..Default::default()};
 let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_none(),"{:?}",flow.error);
}
#[tokio::test]
async fn disabled_modules_and_bad_parameters_fail_cleanly(){
 let (_dir,e,calls)=engine();let mut request=draft();request.scripts=Scripts{enabled:true,modules:capture_core::script_tools::Modules{encoding:false,crypto:false,utils:false},before:"assert(typeof encoding==='undefined');assert(typeof crypto==='undefined');assert(typeof utils==='undefined');assert(md5('abc').length===32);".into(),..Default::default()};
 assert!(e.execute(request,None,"replay",None).await.unwrap().error.is_none());
 for code in ["encoding.hexDecode('f');", "crypto.hash('invalid','x');", "crypto.randomBytes(-1);", "crypto.randomBytes(65537);", "crypto.encrypt({mode:'AES-CBC'});", "crypto.encrypt({mode:'AES-GCM',key:'00',keyEncoding:'hex',nonce:'00',nonceEncoding:'hex'});", "encoding.base64Decode('!!!!');", "encoding.convert('ff',{from:'hex',to:'utf8'});"] {
  let mut request=draft();request.scripts=Scripts{enabled:true,before:code.into(),..Default::default()};let flow=e.execute(request,None,"replay",None).await.unwrap();assert!(flow.error.is_some(),"{code}");
 }
 assert_eq!(calls.lock().unwrap().len(),1);
}
