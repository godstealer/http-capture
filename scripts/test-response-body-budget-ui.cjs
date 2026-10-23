const assert=require('node:assert/strict');
const {build}=require('esbuild');
let active;
const react={
  useState(initial){const h=active,i=h.index++;if(!(i in h.slots))h.slots[i]=initial;return [h.slots[i],value=>{h.slots[i]=typeof value==='function'?value(h.slots[i]):value;h.changed=true}]},
  useEffect(effect,deps){const h=active,i=h.index++,previous=h.slots[i];if(!previous||deps.some((value,j)=>value!==previous[j])){h.effects.push(()=>{h.cleanups[i]?.();h.cleanups[i]=effect()});h.slots[i]=deps}},
  useRef(initial){const h=active,i=h.index++;if(!(i in h.slots))h.slots[i]={current:initial};return h.slots[i]},
  lazy:()=>function CodeView(){},Suspense:()=>null,
};
const jsx=(type,props={})=>({type,props});
const nodes=value=>Array.isArray(value)?value.flatMap(nodes):value&&typeof value==='object'?[value,...nodes(value.props?.children)]:[];
(async()=>{
const output=await build({entryPoints:['apps/frontend/src/ResponseBody.tsx'],bundle:true,platform:'node',format:'cjs',write:false,packages:'external',jsx:'automatic',plugins:[{name:'response-budget',setup(builder){
  builder.onLoad({filter:/[/\\]i18n\.ts$/},()=>({contents:'export const t=value=>value;'}));
  builder.onLoad({filter:/[/\\]ImagePreview\.tsx$/},()=>({contents:'export default function ImagePreview(){}'}));
}}]});
const builtModule={exports:{}};
new Function('module','exports','require',output.outputFiles[0].text)(builtModule,builtModule.exports,name=>{
  if(name==='react')return react;
  if(name==='react/jsx-runtime')return{jsx,jsxs:jsx};
  if(name==='@tauri-apps/api/core')return{invoke:async()=>'',isTauri:()=>false};
  return require(name);
});
const Body=builtModule.exports.default;
const body='A'.repeat(4*Math.ceil((8*1024*1024+1)/3));
const copies=[];let workerStarts=0;
global.Worker=class{constructor(){workerStarts++}postMessage(){throw Error('oversized body reached worker')}terminate(){}};
const h={slots:[],cleanups:[],effects:[],index:0,changed:false};
let tree;
do{h.changed=false;h.index=0;active=h;tree=Body({body,headers:[],onCopy:value=>copies.push(value)});h.effects.splice(0).forEach(effect=>effect())}while(h.changed);
assert.equal(workerStarts,0,'oversized body is rejected before Worker allocation');
const rendered=nodes(tree);
const view=rendered.find(node=>typeof node.type==='function'&&node.type.name==='CodeView');
assert(view,'Base64 prefix is still visible');
assert.equal(view.props.text.length,65536,'CodeView never receives the complete encoded body');
assert(rendered.some(node=>typeof node.props?.children==='string'&&node.props.children.includes('前 64 KiB Base64')));
rendered.find(node=>node.type==='button'&&node.props.children==='复制当前内容').props.onClick();
assert.equal(copies[0],body,'explicit copy retains complete Base64 semantics');
h.cleanups.forEach(cleanup=>cleanup?.());
console.log('Response body budget UI: oversized Base64 avoids Worker and full editor text while explicit copy stays complete.');
})().catch(error=>{console.error(error);process.exitCode=1});
