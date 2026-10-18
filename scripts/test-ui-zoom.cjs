const assert = require('node:assert/strict');
const {buildSync} = require('esbuild');
function load(path) {
  const m = {exports:{}};
  const bundled = buildSync({entryPoints:[path], bundle:true, platform:'node', format:'cjs', write:false, packages:'external', jsx:'automatic'});
  new Function('module','exports','require',bundled.outputFiles[0].text)(m,m.exports,require);
  return m.exports;
}
const {readUiZoom, createZoomQueue, layoutDelta, menuPosition} = load('apps/frontend/src/uiZoom.ts');
const {headerLines, rawHeaderText} = load('apps/frontend/src/messageText.ts');
const close = (actual, expected) => assert.ok(Math.abs(actual-expected)<1e-8, `${actual} != ${expected}`);

(async () => {
  for (const level of [75,90,100,110,125,150]) {
    assert.equal(readUiZoom({getItem:()=>String(level)}),level);
    const scale=level/100;
    close(layoutDelta(30*scale,scale),30);
    const viewport={width:1200,height:900}, size={width:268*scale,height:340*scale};
    const position=menuPosition({x:210,y:160},size,viewport,scale);
    assert.ok(Math.abs(position.left*scale-210)<1e-8);
    assert.ok(Math.abs(position.top*scale-160)<1e-8);
    const edge=menuPosition({x:1198,y:898},size,viewport,scale);
    assert.ok(edge.left*scale+size.width<=1192+1e-8);
    assert.ok(edge.top*scale+size.height<=892+1e-8);
    const outside=menuPosition({x:-20,y:-10},size,viewport,scale);
    close(outside.left*scale,8);close(outside.top*scale,8);
    const oversized=menuPosition({x:100,y:100},{width:2000,height:2000},viewport,scale);
    close(oversized.left*scale,8);close(oversized.top*scale,8);
    close(oversized.maxWidth*scale,1184);close(oversized.maxHeight*scale,884);
  }
  for (const value of [null,'','0','NaN','110.1','200','-75']) assert.equal(readUiZoom({getItem:()=>value}),100);
  assert.equal(readUiZoom({getItem:()=>{throw Error('storage disabled');}}),100);
  const calls=[];let release;
  const queue=createZoomQueue(async scale=>{calls.push(scale);if(scale===0.75)await new Promise(resolve=>{release=resolve;});if(scale===1.1)throw Error('native unavailable');});
  const first=queue(0.75), next=queue(1.5);
  await Promise.resolve();assert.deepEqual(calls,[0.75]);release();await Promise.all([first,next]);
  assert.deepEqual(calls,[0.75,1.5]);
  await assert.rejects(queue(1.1),/native unavailable/);
  await queue(1);assert.deepEqual(calls,[0.75,1.5,1.1,1]);

  const headers=[{name:':method',value:'GET'},{name:'X-Mixed',value:'first'},{name:'x-mixed',value:'second'},{name:'X-Empty',value:''},{name:'X-Text',value:'中文\tvalue'}];
  assert.equal(headerLines(headers),':method: GET\r\nX-Mixed: first\r\nx-mixed: second\r\nX-Empty: \r\nX-Text: 中文\tvalue');
  assert.equal(headerLines([]),'');
  const raw='GET / HTTP/1.1\r\nX-Test: '+String.fromCharCode(0x80,0xff)+'\r\nX-Empty:\r\n\r\n';
  assert.equal(rawHeaderText(Buffer.from(raw,'latin1').toString('base64')),raw);
  for (const value of [undefined,null,'','%%%']) assert.equal(rawHeaderText(value),null);
  const React=require('react'), {renderToStaticMarkup}=require('react-dom/server');
  const {MessageHeaders}=load('apps/frontend/src/RequestDetails.tsx');
  const empty=renderToStaticMarkup(React.createElement(MessageHeaders,{headers:[],onCopy:()=>assert.fail('render must not copy')}));
  assert.match(empty,/<button[^>]*disabled=""/);
  const visible=renderToStaticMarkup(React.createElement(MessageHeaders,{headers,onCopy:()=>{}}));
  assert.equal((visible.match(/<button\b/g)||[]).length,headers.length+1);
  assert.ok(!visible.includes('disabled=""'));
  console.log('UI zoom: all six scales, pointer deltas, menu viewport edges, persistent preference fallback, ordered native zoom and failure recovery passed.');
  console.log('Message copy: exact case/order/duplicate/empty/pseudo headers and lossless raw header bytes passed.');
})().catch(error=>{console.error(error);process.exitCode=1;});
