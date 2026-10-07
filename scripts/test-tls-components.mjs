import fs from 'node:fs';
const root = 'http://127.0.0.1:1420/__capture';
async function api(path, body) {
 const r=await fetch(`${root}/${path}`,{method:body===undefined?'GET':'POST',headers:{'X-Capture-UI':'1','Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(45000)});
 if(!r.ok)throw Error(await r.text());return r.json();
}
const status=await api('status'), caps=status.sendEngines;
let profiles=await api('upstream/profiles');
const http=profiles.find(p=>p.url==='http://127.0.0.1:7897/'||p.url==='http://127.0.0.1:7897');
if(!http)throw Error('Save local HTTP proxy 127.0.0.1:7897 before running this test');
let socks=profiles.find(p=>p.url==='socks5://127.0.0.1:7897/'||p.url==='socks5://127.0.0.1:7897');
if(!socks){await api('upstream/profiles',{input:{name:'系统本地代理 7897（SOCKS5）',config:{enabled:true,url:'socks5://127.0.0.1:7897',username:'',password:'',authEnabled:false}}});profiles=await api('upstream/profiles');socks=profiles.find(p=>p.url.startsWith('socks5://127.0.0.1:7897'));}
const base=JSON.parse(fs.readFileSync('.local/tls-peet-request.json','utf8').replace(/^\uFEFF/,''));
const cases=[];
function add(engine,preset,version,route,tls={}){cases.push({engine,preset,version,route,tls});}
for(const e of ['native','auto','h2'])for(const r of ['HTTP','SOCKS5'])add(e,'native',null,r);
add('auto','native',null,'direct');add('h3','native',null,'direct');add('h3','native',null,'HTTP');
add('h3','native',null,'SOCKS5');
for(const e of caps.filter(e=>['wreq','httpcloak'].includes(e.id)&&e.available)) {
 for(const [family,versions] of Object.entries(e.browserVersions)){
  if(e.id==='httpcloak'){for(const v of versions)add(e.id,family,String(v),'HTTP');add(e.id,family,String(versions[0]),'SOCKS5');}
  else{for(const route of ['direct','HTTP','SOCKS5'])add(e.id,family,String(versions[0]),route);}
 }
}
add('httpcloak','chrome','auto','HTTP');add('httpcloak','chrome','999','HTTP');
for(const v of ['1.2','1.3'])add('native','native',null,'HTTP',{version:v});
const results=[];
async function run(c){
 const request={...structuredClone(base),url:c.engine==='h3'?'https://tls3.peet.ws/api/all':'https://tls.peet.ws/api/all',engine:c.engine,upstreamProfileId:c.route==='direct'?null:c.route==='HTTP'?http.id:socks.id,tls:{preset:c.preset,browserVersion:c.version,...c.tls},scripts:{enabled:false}};
 const started=Date.now();let item={...c};
 try{
  const flow=await api('replay',{request});if(flow.error)throw Error(flow.error);
  const body=JSON.parse(Buffer.from(flow.response.bodyBase64,'base64').toString('utf8'));
  const ua=request.headers.find(h=>h.name.toLowerCase()==='user-agent').value;
  if(flow.response.status!==200||!body.tls||body.user_agent!==ua)throw Error('Status / TLS echo / UA mismatch');
  if(c.engine==='h3'&&(flow.response.version!=='HTTP/3'||body.http_version!=='h3'||!body.http3))throw Error('HTTP/3 protocol / server echo mismatch');
  const actual=(body.http2?.sent_frames??[]).find(f=>f.frame_type==='HEADERS')?.headers??[];
  const wanted=request.headers.map(h=>`${h.name.toLowerCase()}: ${h.value}`);
  const observed=actual.filter(h=>wanted.includes(h));
  item={...item,result:'PASS',status:flow.response.status,protocol:flow.response.version,tlsVersion:flow.response.tlsVersion,certificates:flow.response.upstreamTls?.certificates?.length??0,headerOrder:actual.length?JSON.stringify(observed)===JSON.stringify(wanted):null,uaPreserved:true};
 }catch(e){const error=String(e.message).replace(/\b(?:\d{1,3}\.){3}\d{1,3}:?\d*/g,'[address]');item={...item,result:/不支持|not available/.test(error)?'UNSUPPORTED':'FAIL',error};}
 item.ms=Date.now()-started;results.push(item);console.log(JSON.stringify(item));
}
const h3Only=process.argv.includes('--h3');
const selectedCases=h3Only?cases.filter(c=>c.engine==='h3'):cases;
let i=0;await Promise.all(Array.from({length:3},async()=>{while(i<selectedCases.length)await run(selectedCases[i++]);}));
fs.writeFileSync(h3Only?'.local/h3-components-results.json':'.local/tls-components-results.json',JSON.stringify(results,null,2));
const rows=results.map(r=>`| ${r.engine} | ${r.preset} ${r.version??r.tls.version??''} | ${r.route} | ${r.result} | ${r.status??''} ${r.protocol??''} ${r.tlsVersion??''} | ${(r.error??'').replaceAll('|','/')} |`);
fs.writeFileSync(h3Only?'docs/h3-matrix-generated.md':'docs/tls-matrix-generated.md',`# TLS 组件实测（脚本生成）\n\n时间：${new Date().toISOString()}\n\n目标：H3 使用 https://tls3.peet.ws/api/all ，其他引擎使用 https://tls.peet.ws/api/all 。通过前端 /__capture/replay 调用；UA 使用用户提供的 Chrome 152。HTTP/SOCKS5 使用本机 7897 代理。${h3Only?'本次只运行 H3 用例。':'httpcloak 覆盖全部公布版本；wreq 每个浏览器抽测最新版本，未逐个验证旧版本。'}\n\n| 引擎 | 预设 | 路径 | 结果 | 响应 | 错误 |\n|---|---|---|---|---|---|\n${rows.join('\n')}\n\nPASS 表示返回 200、可解析 TLS 回显且 UA 未被改写；H3 额外验证本地协议与服务器 h3 回显。不代表完整浏览器指纹保真。H3 的普通 HTTP 上游为预期拒绝用例。GUI 点击及 TUN 未在此脚本中验证。\n`);
console.log(`TOTAL ${results.length}, PASS ${results.filter(r=>r.result==='PASS').length}, FAIL ${results.filter(r=>r.result==='FAIL').length}, UNSUPPORTED ${results.filter(r=>r.result==='UNSUPPORTED').length}`);
