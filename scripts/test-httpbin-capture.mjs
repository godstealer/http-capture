import http from 'node:http';
import tls from 'node:tls';
import fs from 'node:fs';
const connect = http.request({ host: '127.0.0.1', port: 8080, method: 'CONNECT', path: 'httpbin.org:443' });
connect.on('error', error => { console.error(error.message); process.exitCode = 1; });
connect.on('connect', (response, socket, head) => {
  if (response.statusCode !== 200) { socket.destroy(); throw new Error(`CONNECT ${response.statusCode}`); }
  if (head.length) socket.unshift(head);
  const client = tls.connect({ socket, servername: 'httpbin.org', ca: fs.readFileSync('.local/capture/certificates/capture-ca.pem'), ALPNProtocols: ['http/1.1'] }, () => {
    client.write('GET /get?capture_test=proxy HTTP/1.1\r\nHost: httpbin.org\r\nX-Capture-Test: proxy-integration\r\nConnection: close\r\n\r\n');
  });
  let result = '';
  client.setTimeout(35000, () => client.destroy(new Error('Capture timed out')));
  client.on('data', data => { result += data.toString('utf8'); });
  client.on('error', error => { console.error(error.message); process.exitCode = 1; });
  client.on('end', () => {
    const valid = result.startsWith('HTTP/1.1 200') && result.includes('"capture_test": "proxy"');
    console.log(JSON.stringify({ httpsMitm: valid, certificateVerified: client.authorized }));
    if (!valid || !client.authorized) process.exitCode = 1;
  });
});
connect.end();
