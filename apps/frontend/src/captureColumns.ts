export const captureColumns = [
  {
    "id": "indicator",
    "label": "连接状态",
    "width": 32,
    "default": true
  },
  {
    "id": "id",
    "label": "ID",
    "width": 60,
    "default": true
  },
  {
    "id": "icon",
    "label": "图标",
    "width": 48,
    "default": true
  },
  {
    "id": "method",
    "label": "方法",
    "width": 80,
    "default": true
  },
  {
    "id": "url",
    "label": "URL",
    "width": 360,
    "default": true
  },
  {
    "id": "type",
    "label": "Type",
    "width": 90,
    "default": true
  },
  {
    "id": "status",
    "label": "状态",
    "width": 65,
    "default": true
  },
  {
    "id": "clientProtocol",
    "label": "客户端协议",
    "width": 100,
    "default": true
  },
  {
    "id": "upstreamProtocol",
    "label": "上游协议",
    "width": 100,
    "default": true
  },
  {
    "id": "tls",
    "label": "TLS（上游）",
    "width": 100,
    "default": true
  },
  {
    "id": "duration",
    "label": "时长",
    "width": 80,
    "default": true
  },
  {
    "id": "size",
    "label": "响应正文大小",
    "width": 110,
    "default": true
  },
  {
    "id": "host",
    "label": "域名",
    "width": 180,
    "default": false
  },
  {
    "id": "path",
    "label": "路径",
    "width": 260,
    "default": false
  },
  {
    "id": "scheme",
    "label": "URL 协议",
    "width": 90,
    "default": false
  },
  {
    "id": "mime",
    "label": "Content-Type",
    "width": 200,
    "default": false
  },
  {
    "id": "started",
    "label": "开始时间",
    "width": 190,
    "default": false
  },
  {
    "id": "requestSize",
    "label": "请求正文大小",
    "width": 110,
    "default": false
  },
  {
    "id": "encoding",
    "label": "Content-Encoding",
    "width": 150,
    "default": false
  },
  {
    "id": "cipher",
    "label": "密码套件（上游）",
    "width": 240,
    "default": false
  },
  {
    "id": "error",
    "label": "错误信息",
    "width": 260,
    "default": false
  }
];
const key = 'http-capture.capture.columns.v1';
export function readColumns(): string[] {
  try {
    const stored: unknown = JSON.parse(localStorage.getItem(key) ?? 'null');
    if (Array.isArray(stored)) {
      const valid = captureColumns.filter(c => stored.includes(c.id)).map(c => c.id);
      if (valid.length) return valid;
    }
  } catch { /* Missing storage or invalid preferences use defaults. */ }
  return captureColumns.filter(c => c.default).map(c => c.id);
}
export function saveColumns(columns: string[]) {
  try { localStorage.setItem(key, JSON.stringify(columns)); } catch { /* Preference remains active for this session. */ }
}
