// Node's HTTP/2 server uses nghttp2, independent of our Rust h2 implementation.
const http2=require('node:http2'),fs=require('node:fs');
const target=process.argv[2];if(!target)throw Error('Pass output JSON path');
const server=http2.createServer();server.on('stream',(stream,headers,flags,rawHeaders)=>{
 const fields=[];for(let i=0;i<rawHeaders.length;i+=2)fields.push({name:rawHeaders[i],value:rawHeaders[i+1]});
 fs.writeFileSync(target,JSON.stringify({nghttp2:process.versions.nghttp2,fields},null,2));
 stream.respond({':status':200});stream.end('OK');
});server.listen(0,'127.0.0.1',()=>console.log('h2c observer: http://127.0.0.1:'+server.address().port));
