# gRPC 调试

当前实现参考用户提供的两篇 Reqable 3.3 文章，支持原生 gRPC over HTTP/2。

## 使用

1. 新建请求，输入服务地址（HTTPS，或支持 prior knowledge 的 HTTP/h2c），TLS 选择 Native。
2. 打开 **gRPC** 标签，导入主 `.proto` 及全部依赖文件，或粘贴定义并保存。也可以点击 **服务反射**，读取服务器的 v1 / v1alpha reflection 服务；服务器必须启用反射。
3. 选择服务/方法，生成示例并编辑 JSON。`int64` 使用字符串防止精度丢失，`bytes` 使用 Base64。
4. 在请求头中设置业务 metadata，例如 authorization，然后点击 **开始 gRPC 调用**。
5. Unary 自动结束请求流；Server stream 持续接收多条消息；Client stream / Bidi 可以多次 **发送消息**，最后点击 **结束请求流**。结束请求流不等于取消响应；**取消调用** 才会终止整个调用。
6. 响应显示 gRPC 状态、metadata、trailers 和消息列表。折叠时显示摘要，展开查看完整 JSON。HTTP 200 不代表 gRPC 成功，以 grpc-status 为准。

定义保存在当前本机前端的 localStorage；反射结果转换为可再次加载的 schema JSON。没有定义时按 protobuf 字段编号和 wire value 展示，不猜测字段名。支持 identity / gzip 消息解码。请求和响应的 trailers 分开保存，响应头不会混入 trailers。

## 已验证

- 本地真实 HTTP/2 服务：一元、服务端流、客户端流、双向流以及服务反射，均通过界面验证。
- 双向流在请求尚未半关闭时收到响应；继续发送第二条消息后，半关闭请求，获得两条响应和最终 grpc-status: 0。
- Rust 集成测试覆盖帧往返、trailers、双向流、半关闭及关闭后的发送失败。
- 前端测试覆盖 proto 方法、JSON/int64/bytes、gzip、无 schema 解析、坏帧和状态。

## 当前边界与后续

- 主动交互流使用 Native HTTP/2，暂不支持 wreq/httpcloak、gRPC-Web、HTTP/3。
- 实时交互调用暂不执行脚本和拦截规则；启用请求脚本会明确拒绝。需要修改消息时，在发送前编辑 JSON。普通 HTTP 发送路径与实时会话是不同入口。
- 捕获的完整 gRPC 消息可解码，H2 响应 trailers 可保留和转发；抓包代理仍会缓冲请求，实时双向流透明转发尚未完成。
- 当前只有一套全局 schema，尚无按服务管理多套定义、普通 Protobuf URL 类型映射、bytes 文件助手。
- 实时入口暂未接入环境变量替换；请使用实际值。一次会话最长 300 秒，后续单次发送最大 16 MiB；长流累计消息目前驻留内存并更新历史，需进一步优化。
- 切换同一请求的子标签不会终止调用；关闭或切换请求工作区会销毁当前面板并取消尚在进行的会话。
- 本次验证使用本地服务，未据此宣称所有第三方服务器、认证方式和跨平台桌面运行均已验证。
