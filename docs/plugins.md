# 本地插件 API v1

首版是本工具的 JavaScript 扩展协议，不兼容 Burp Java/Montoya/Jython 插件。

## 安装和使用

打开工作区「插件」，导入 JSON 插件包，查看代码后点击安装。新安装插件始终禁用，勾选「启用插件」才运行。支持上移调整执行顺序和卸载；配置、代码、启用状态保存在本地数据库。相同 ID 不覆盖，升级前先卸载旧版本。日志面板点击刷新更新，最多保留本进程最近 200 条；具体请求的发送说明也保留插件执行记录。

示例：[demo-header.json](../examples/plugins/demo-header.json)，只匹配本机 127.0.0.1 HTTP 请求，分别添加请求与响应头。

## 清单

```json
{
  "id": "my-plugin",
  "name": "My plugin",
  "version": "1.0.0",
  "apiVersion": 1,
  "enabled": false,
  "scope": "replay",
  "urlPattern": "^https://example\\.com/",
  "before": "request.headers.push({name:'X-Demo',value:'hello'}); console.log('sent');",
  "after": "response.headers.push({name:'X-Modified',value:'1'});"
}
```

scope 可为 capture、replay、all。URL 使用 Rust regex 语法；匹配原始 URL，在一次调用开始时确定插件快照和顺序，运行中修改配置对下一次请求生效。最多 32 个插件，每段代码最多 64 KiB。

## 钩子

每次钩子在独立 QuickJS 上下文执行，使用既有脚本 API：request、response、console.log、encodeText、decodeText、哈希和内置工具库。headers 是有序 name/value 数组，bodyBase64 保存原始字节。before 修改请求，after 修改响应；相反方向的修改不提交。variables 仅在本次钩子有效，不跨阶段或请求持久化。

执行顺序为插件请求钩子 → 请求脚本 → 请求断点 → 上游发送 → 插件响应钩子 → 响应脚本 → 响应断点。错误或输出校验失败时丢弃该钩子的修改并继续后续插件。原始请求和响应另行保存。断点直接替换响应时不会执行上游响应钩子。

## 运行边界

- 插件可读取匹配流量全部内容并修改请求目的地址，因此只启用可信代码。沙箱不提供独立网络、文件系统、进程、require 或浏览器 DOM。
- 每个钩子限制 2 秒、64 MiB 内存、256 KiB 栈，沿用脚本执行器最多 4 个并发任务；忙时记录失败并继续原请求。同步 JavaScript，不支持 async/Promise。
- 沿用现有脚本输出校验限制（响应正文最大 8 MiB 等），不适合作为大文件转换器。
- 普通 HTTP 抓包和手动发送共用钩子。WebSocket 握手/消息、主动交互式 gRPC 和 TUN 特殊流量尚未接入，不能据此认为所有流量都会调用插件。
- 匹配带 after 钩子的 SSE 实时响应将被当前流式发送保护拒绝，避免发送后再修改响应；请用 URL 规则排除这类流量。
- 尚无扩展 UI 面板、右键菜单注册、扫描器、市场、自动升级、签名校验和细粒度能力授权。当前权限是固定的流量读写沙箱，不声称具备完整 Burp 扩展能力。

## 验证

Rust 测试覆盖 capture/replay 请求响应修改、插件顺序、原始记录、错误回滚、URL 匹配、禁用、重复 ID/API 版本拒绝及落盘恢复；已有脚本的 7 项测试通过。桌面和服务端编译及前端生产构建通过。
