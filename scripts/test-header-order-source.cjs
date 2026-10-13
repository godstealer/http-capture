const {buildSync}=require('esbuild');const assert=require('node:assert/strict');
const m={exports:{}};new Function('module','exports',buildSync({entryPoints:['apps/frontend/src/headerOrderSource.ts'],bundle:true,platform:'node',format:'cjs',write:false}).outputFiles[0].text)(m,m.exports);
const flow={source:'capture',clientProtocol:'HTTP/2',request:{headers:[{name:'Wrong',value:'edited'}]},notes:[],rawRequestHeadBase64:Buffer.from('GET / HTTP/1.1\r\nX-A: 1\r\nx-b: 2\r\nX-A: 3\r\n\r\n').toString('base64')};
assert.deepEqual(m.exports.headerOrderSource(flow),{original:true,headers:[{name:'X-A',value:'1'},{name:'x-b',value:'2'},{name:'X-A',value:'3'}]});
delete flow.rawRequestHeadBase64;flow.request.pseudoHeaders=[{name:':method',value:'GET'}];assert.equal(m.exports.headerOrderSource(flow).original,true);flow.state='imported';assert.equal(m.exports.headerOrderSource(flow).original,false);
console.log('Header order: raw H1 wins over edited draft; duplicate/case/order preserved; imported H2 not claimed as observed wire order.');

flow.state='completed';flow.source='replay';assert.equal(m.exports.headerOrderSource(flow).original,false);
flow.source='capture';flow.clientProtocol='HTTP/3';assert.equal(m.exports.headerOrderSource(flow).original,false);
flow.clientProtocol='HTTP/2';flow.rawRequestHeadBase64=Buffer.from('garbage').toString('base64');delete flow.request.pseudoHeaders;assert.equal(m.exports.headerOrderSource(flow).original,false);
