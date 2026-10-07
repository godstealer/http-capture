use anyhow::{Result, ensure, bail};
use base64::{Engine as _,engine::general_purpose::{STANDARD,URL_SAFE,URL_SAFE_NO_PAD}};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use sha2::{Digest,Sha256,Sha512};
use sha1::Sha1;
use md5::Md5;
use hmac::{Hmac,Mac};
use aes_gcm::{Aes128Gcm,Aes256Gcm,Nonce,aead::{Aead,KeyInit,Payload}};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(default,deny_unknown_fields)]
pub struct Modules{pub encoding:bool,pub crypto:bool,pub utils:bool}
impl Default for Modules{fn default()->Self{Self{encoding:true,crypto:true,utils:true}}}
fn text<'a>(v:&'a Value,k:&str)->Result<&'a str>{v.get(k).and_then(Value::as_str).ok_or_else(||anyhow::anyhow!("缺少字符串参数 {k}"))}
fn option<'a>(v:&'a Value,k:&str,default:&'a str)->Result<&'a str>{match v.get(k){None=>Ok(default),Some(Value::String(s))=>Ok(s),_=>bail!("{k} 必须是字符串")}}
fn decode(data:&str,format:&str)->Result<Vec<u8>>{
 ensure!(data.len()<=12*1024*1024,"输入过大");
 Ok(match format {
 "utf8"=>data.as_bytes().to_vec(),"base64"=>STANDARD.decode(data)?,"base64url"=>if data.contains('='){URL_SAFE.decode(data)?}else{URL_SAFE_NO_PAD.decode(data)?},
 "hex"=>{ensure!(data.len()%2==0 && data.bytes().all(|b|b.is_ascii_hexdigit()),"无效 Hex：必须是偶数位十六进制字符");(0..data.len()).step_by(2).map(|i|u8::from_str_radix(&data[i..i+2],16)).collect::<std::result::Result<Vec<_>,_>>()?},_=>bail!("不支持的编码 {format}")})
}
fn encode(data:&[u8],format:&str)->Result<String>{Ok(match format{"utf8"=>String::from_utf8(data.to_vec())?,"base64"=>STANDARD.encode(data),"base64url"=>URL_SAFE_NO_PAD.encode(data),"hex"=>data.iter().map(|b|format!("{b:02x}")).collect(),_=>bail!("不支持的编码 {format}")})}
pub fn call(modules:&Modules,namespace:&str,method:&str,args:&str)->Result<String>{
 let v:Value=serde_json::from_str(args)?;
 let result=match namespace {
 "encoding"=>{ensure!(modules.encoding,"encoding 模块未启用");ensure!(method=="convert","未知编码函数");json!(encode(&decode(text(&v,"input")?,text(&v,"from")?)?,text(&v,"to")?)?)},
 "utils"=>{ensure!(modules.utils,"utils 模块未启用");match method{
 "uuid"=>json!(uuid::Uuid::new_v4().to_string()),
 "timestamp"=>{let d=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?;json!(match option(&v,"unit","seconds")?{"seconds"=>d.as_secs(),"milliseconds"=>d.as_millis() as u64,_=>bail!("时间戳单位应为 seconds 或 milliseconds")})},_=>bail!("未知工具函数")}},
 "crypto"=>{ensure!(modules.crypto,"crypto 模块未启用");match method{
 "randomBytes"=>{let n=v.get("length").and_then(Value::as_u64).ok_or_else(||anyhow::anyhow!("length 必须是正整数"))?;ensure!((1..=65536).contains(&n),"随机字节长度为 1–65536");let mut bytes=vec![0;n as usize];getrandom::getrandom(&mut bytes).map_err(|e|anyhow::anyhow!("随机数生成失败：{e}"))?;json!(encode(&bytes,option(&v,"output","hex")?)?)},
 "hash"|"hmac"=>{let data=decode(text(&v,"input")?,option(&v,"inputEncoding","utf8")?)?;let algorithm=text(&v,"algorithm")?.to_ascii_uppercase().replace('-',"");
 let bytes=if method=="hash"{match algorithm.as_str(){"MD5"=>Md5::digest(&data).to_vec(),"SHA1"=>Sha1::digest(&data).to_vec(),"SHA256"=>Sha256::digest(&data).to_vec(),"SHA512"=>Sha512::digest(&data).to_vec(),_=>bail!("不支持的摘要算法")}}
 else{let key=decode(text(&v,"key")?,option(&v,"keyEncoding","utf8")?)?;macro_rules! sign{($t:ty)=>{{let mut m=<Hmac<$t> as Mac>::new_from_slice(&key)?;m.update(&data);m.finalize().into_bytes().to_vec()}}}match algorithm.as_str(){"MD5"=>sign!(Md5),"SHA1"=>sign!(Sha1),"SHA256"=>sign!(Sha256),"SHA512"=>sign!(Sha512),_=>bail!("不支持的 HMAC 算法")}};json!(encode(&bytes,option(&v,"output","hex")?)?)},
 "encrypt"|"decrypt"=>{ensure!(text(&v,"mode")?=="AES-GCM","当前支持 AES-GCM（128/256 位密钥）");
 let key=decode(text(&v,"key")?,text(&v,"keyEncoding")?)?;let nonce=decode(text(&v,"nonce")?,text(&v,"nonceEncoding")?)?;ensure!(nonce.len()==12,"AES-GCM nonce 必须为 12 字节");ensure!(key.len()==16||key.len()==32,"AES-GCM 密钥必须为 16 或 32 字节");
 let data=decode(text(&v,"data")?,text(&v,"inputEncoding")?)?;let aad=decode(option(&v,"aad","")?,option(&v,"aadEncoding","utf8")?)?;let payload=Payload{msg:&data,aad:&aad};
 macro_rules! crypt{($t:ty)=>{{let cipher=<$t>::new_from_slice(&key).map_err(|_|anyhow::anyhow!("无效 AES 密钥"))?;if method=="encrypt"{cipher.encrypt(Nonce::from_slice(&nonce),payload)}else{cipher.decrypt(Nonce::from_slice(&nonce),payload)}}}}
 let result=if key.len()==16{crypt!(Aes128Gcm)}else{crypt!(Aes256Gcm)}.map_err(|_|anyhow::anyhow!("AES-GCM 处理失败：密钥、nonce、AAD 或认证标签不匹配"))?;
 json!(encode(&result,text(&v,"output")?)?)},_=>bail!("未知密码函数")}},_=>bail!("未知模块")};Ok(serde_json::to_string(&result)?)
}
pub const BOOTSTRAP:&str=r#"
const invokeTool=(module,method,args)=>JSON.parse(__tool(module,method,JSON.stringify(args)));
if (__modules.encoding) globalThis.encoding=Object.freeze({
 convert:(input,{from,to})=>invokeTool('encoding','convert',{input,from,to}),
 base64Encode:(input,from='utf8')=>invokeTool('encoding','convert',{input,from,to:'base64'}),
 base64Decode:(input,to='utf8')=>invokeTool('encoding','convert',{input,from:'base64',to}),
 base64urlEncode:(input,from='utf8')=>invokeTool('encoding','convert',{input,from,to:'base64url'}),
 base64urlDecode:(input,to='utf8')=>invokeTool('encoding','convert',{input,from:'base64url',to}),
 hexEncode:(input,from='utf8')=>invokeTool('encoding','convert',{input,from,to:'hex'}),
 hexDecode:(input,to='utf8')=>invokeTool('encoding','convert',{input,from:'hex',to}),
});
if (__modules.crypto) globalThis.crypto=Object.freeze({
 hash:(algorithm,input,options={})=>invokeTool('crypto','hash',{...options,algorithm,input}),
 hmac:(algorithm,{message,...options})=>invokeTool('crypto','hmac',{...options,algorithm,input:message}),
 randomBytes:(length,output='hex')=>invokeTool('crypto','randomBytes',{length,output}),
 encrypt:options=>invokeTool('crypto','encrypt',options),
 decrypt:options=>invokeTool('crypto','decrypt',options),
});
if (__modules.utils) globalThis.utils=Object.freeze({timestamp:(unit='seconds')=>invokeTool('utils','timestamp',{unit}),uuid:()=>invokeTool('utils','uuid',{})});
"#;
