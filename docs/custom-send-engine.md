# 自定义发送引擎（协议 v1）

HTTP Capture 可在手动请求中选择 `custom` 发送引擎。它通过标准输入/输出与用户明确指定的本地程序交换 JSON；每次请求启动一个独立进程，取消请求时结束该进程。没有配置时不会查找或运行其他程序。抓包代理转发仍使用内置链路。

## 配置

将可执行程序和同名 `.json` 能力清单放在一起，启动 HTTP Capture 前将环境变量 `HTTP_CAPTURE_CUSTOM_ENGINE` 设为**可执行文件的绝对路径**。例如 Windows 的 `C:\Tools\http-capture-custom.exe` 对应 `C:\Tools\http-capture-custom.json`。重启应用后，在手动请求的「设置 → 发送引擎」选择 `custom`。只运行你信任的程序；请求头、正文、上游代理凭据和被允许的客户端证书可能通过标准输入交给它。

能力清单示例见 [http-capture-custom.json](../examples/custom-send-engine/http-capture-custom.json)：

```json
{
  "protocolVersion": 1,
  "profiles": ["native"],
  "browserVersions": {},
  "customClientHello": false,
  "certificateRoots": false,
  "clientCertificate": false
}
```

`profiles` 必须是 `native`、`chrome`、`firefox` 中的 1–3 个不重复值；`browserVersions` 的键只能来自 `profiles`。如程序支持指定浏览器版本，可填写例如 `{"chrome":[155]}`。只有程序确实实现相应验证和握手能力时才将后三个开关设为 `true`；不支持的证书或 ClientHello 配置会在启动子进程前报错。

## 请求与响应

进程从标准输入读取**一个** JSON 文档直至 EOF，并向标准输出写入**一个** JSON 文档后退出。标准错误输出不进入请求记录。请求字段和响应字段采用 camelCase；正文为原始字节的 Base64。字段顺序、重复请求头和大小写保留为数组，不要转成对象。

输入结构：

```json
{
  "request": {
    "engine": "custom", "method": "GET", "url": "https://example.com/",
    "headers": [{"name": "Accept", "value": "application/json"}],
    "pseudoHeaders": [], "bodyBase64": "", "tls": {"preset": "native"},
    "upstreamProfileId": null, "scripts": {"enabled": false}
  },
  "preset": "native",
  "proxy": null,
  "security": null
}
```

`request` 是经过应用脚本和 HTTP 字段校验后的请求。`proxy` 存在时包含 `url`、`username`、`password`，程序必须通过该代理发送，不能失败后改为直连。`security` 存在时可能包含 `trustedCaPem`、`clientCertificatePem`、`clientPrivateKeyPem`；程序必须验证服务端证书，匹配配置时使用指定的客户端身份。未实现这些功能应保持能力位为 `false`。程序应发送一次请求并返回收到的首个 3xx，由应用的手动发送开关决定是否继续跳转。

成功输出：

```json
{
  "response": {
    "status": 200, "version": "HTTP/1.1", "headers": [],
    "bodyBase64": "", "rawHeadBase64": null
  },
  "notes": [], "error": null
}
```

失败时输出 `{"response":null,"notes":[],"error":"具体错误"}`。成功状态码必须为 100–599，协议版本文本不超过 32 字符。`response` 还可包含 `trailers`、`tlsVersion`、`upstreamTls`、`sentRequestHeaders`；只有实际观察到的值才应填写。进程必须以退出码 0 正常结束；否则应用按发送失败处理。

仓内 [Go 示例](../examples/custom-send-engine/main.go) 只回显固定响应，不进行网络连接。可在示例目录运行 `go build -o http-capture-custom.exe main.go`（Windows）或 `go build -o http-capture-custom main.go`（macOS/Linux），再按上文配置路径。它用于验证协议接线，不能充当真实 HTTP/TLS 客户端。

自定义程序由用户负责维护和更新；HTTP Capture 不下载、不扫描目录安装，也不会把它打包进正式安装包。
