import { encodeText, type Flow } from './types';

const paths = [
  ['GET', '/v1/projects', 200, 124],
  ['GET', '/v1/user/profile', 200, 86],
  ['POST', '/v1/events', 201, 208],
  ['GET', '/v1/projects/atlas/members', 200, 97],
  ['OPTIONS', '/v1/events', 204, 32],
  ['GET', '/v1/notifications', 304, 54],
  ['GET', '/v1/usage', 200, 112],
  ['POST', '/v1/auth/refresh', 401, 65],
] as const;
export const demoFlows: Flow[] = paths.map(([method, path, status, time], index) => ({
  id: `demo-${index}`, parentId: null, startedAt: Date.now() - index * 18000,
  durationMs: time, source: 'demo', error: null,
  request: { method, url: `https://api.example.com${path}`, tls: { preset: 'chrome' },
    headers: [
      { name: 'Host', value: 'api.example.com' },
      { name: 'Connection', value: 'keep-alive' },
      { name: 'sec-ch-ua', value: '"Chromium";v="147", "Google Chrome";v="147"' },
      { name: 'Accept', value: 'application/json' },
      { name: 'Authorization', value: 'Bearer demo_token' },
      { name: 'User-Agent', value: 'Mozilla/5.0 Chrome/147.0.0.0 Safari/537.36' },
      { name: 'Accept-Encoding', value: 'gzip, deflate, br' },
      { name: 'Accept-Language', value: 'zh-CN,zh;q=0.9' },
    ], bodyBase64: method === 'POST' ? encodeText('{\n  "event": "page_view",\n  "project": "atlas"\n}') : '' },
  response: { status, version: 'HTTP/2', headers: [
    { name: 'content-type', value: 'application/json; charset=utf-8' },
    { name: 'cache-control', value: 'no-cache' },
    { name: 'x-request-id', value: 'req_demo_8f2a9c' },
  ], bodyBase64: encodeText(status === 204 || status === 304 ? '' : JSON.stringify(index === 0 ? {
    data: [
      { id: 'prj_atlas', name: 'Atlas', description: 'Design system & component library', status: 'active', members: 12 },
      { id: 'prj_orbit', name: 'Orbit', description: 'Customer dashboard', status: 'active', members: 8 },
    ], meta: { total: 2, page: 1, has_more: false },
  } : { success: status < 400, message: status === 401 ? 'Token expired' : 'Demo response' }, null, 2)) },
  notes: ['这是一条界面演示数据，未发送网络请求。'],
}));
