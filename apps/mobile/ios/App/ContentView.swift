import SwiftUI
import UniformTypeIdentifiers
import Combine

struct ContentView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @State private var showingManualRequest = false
    @State private var showingRules = false
    @State private var showingResponseRules = false
    @State private var showingTrustedCertificates = false
    @State private var portInput = ""

    private var validPort: Int? {
        guard let port = Int(portInput), (1...65535).contains(port) else { return nil }
        return port
    }

    var body: some View {
        NavigationStack {
            List {
                Section("抓包") {
                    HStack {
                        Text("监听端口")
                        Spacer()
                        TextField("端口", text: $portInput)
                            .keyboardType(.numberPad)
                            .multilineTextAlignment(.trailing)
                            .frame(width: 100)
                            .disabled(tunnel.shouldOfferStop || tunnel.busy || tunnel.tunnelStopping)
                            .onChange(of: portInput) { value in
                                let digits = value.filter { $0 >= "0" && $0 <= "9" }
                                if digits != value {
                                    portInput = digits
                                } else if let port = validPort {
                                    tunnel.port = port
                                }
                            }
                    }
                    if validPort == nil {
                        Text("端口必须为 1–65535")
                            .font(.footnote).foregroundStyle(.red)
                    }
                    Toggle("尝试抓取 HTTP/3（UDP/443）", isOn: $tunnel.captureQuic)
                        .disabled(tunnel.shouldOfferStop || tunnel.busy || tunnel.tunnelStopping)
                    Text("开启后需信任本 CA；ECH 或隐藏 SNI 的连接仍不能解密。其他 UDP 保持直连。")
                        .font(.footnote).foregroundStyle(.secondary)
                    Text(tunnel.message).foregroundStyle(.secondary)
                    Button(tunnel.shouldOfferStop ? "停止抓包" : "启动抓包") {
                        if tunnel.shouldOfferStop { tunnel.stop() } else { tunnel.start() }
                    }.disabled(tunnel.busy || tunnel.tunnelStopping || (!tunnel.shouldOfferStop && validPort == nil))
                }
                Section("证书") {
                    Button("导出 CA 公钥证书") { tunnel.exportCertificate() }
                        .disabled(!tunnel.connected)
                    if let url = tunnel.certificateURL {
                        ShareLink("分享或保存证书", item: url)
                    }
                    Button("上游 CA 证书") { showingTrustedCertificates = true }
                        .disabled(!tunnel.connected)
                    Text("HTTPS 解密需要在设备设置中安装并信任该 CA；证书固定的应用可能拒绝解密。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("请求记录") {
                    HStack {
                        TextField("按 URL 搜索", text: $tunnel.urlFilterInput)
                            .textInputAutocapitalization(.never).autocorrectionDisabled()
                        Button("搜索") { tunnel.applyUrlFilter() }
                        Button("清除") {
                            tunnel.urlFilterInput = ""
                            tunnel.applyUrlFilter()
                        }
                    }
                    .buttonStyle(.borderless)
                    .disabled(!tunnel.connected)
                    HStack {
                        Button("刷新") { tunnel.refreshRequests() }
                        Spacer()
                        Button("新建请求") { showingManualRequest = true }
                    }
                    .buttonStyle(.borderless)
                    .disabled(!tunnel.connected)
                    ForEach(tunnel.requests) { request in
                        Button { tunnel.showDetail(id: request.id) } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                HStack {
                                    Text(request.method).bold()
                                    if let status = request.status { Text(String(status)).foregroundStyle(.secondary) }
                                }
                                Text(request.url).lineLimit(2).font(.footnote)
                            }
                        }
                    }
                    HStack {
                        Button("上一页") { tunnel.previousRequestsPage() }
                            .disabled(!tunnel.connected || !tunnel.hasPreviousRequests || tunnel.requestPageLoading)
                        Spacer()
                        Text("第 \(tunnel.requestPageNumber) 页")
                            .font(.footnote).foregroundStyle(.secondary)
                        Spacer()
                        Button("下一页") { tunnel.refreshRequests(loadMore: true) }
                            .disabled(!tunnel.connected || !tunnel.hasMoreRequests || tunnel.requestPageLoading)
                    }
                    .buttonStyle(.borderless)
                    if tunnel.requestPageLoading { ProgressView("正在读取请求记录…") }
                }
                Section("规则") {
                    Button("管理请求阻止规则") { showingRules = true }
                        .disabled(!tunnel.connected)
                    Button("管理响应修改规则") { showingResponseRules = true }
                        .disabled(!tunnel.connected)
                }
            }
            .navigationTitle("HTTP Capture")
            .onAppear { portInput = String(tunnel.port) }
            .onChange(of: tunnel.port) { port in
                if validPort != port { portInput = String(port) }
            }
            .onReceive(Timer.publish(every: 2, on: .main, in: .common).autoconnect()) { _ in
                tunnel.pollRequests()
            }
            .sheet(isPresented: Binding(get: { tunnel.detailOpen },
                                        set: { if !$0 { tunnel.clearDetail() } })) {
                NavigationStack {
                    ScrollView {
                        Text(tunnel.selectedDetail)
                            .font(.system(.footnote, design: .monospaced))
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .padding()
                            .textSelection(.enabled)
                    }
                    .id(tunnel.bodyVariant.map { "\($0)-\(tunnel.bodyOffset)" } ?? "detail")
                    .navigationTitle(tunnel.bodyVariant == nil ? "请求详情" : "正文 · \(tunnel.bodyPosition)")
                    .toolbar { Button("关闭") { tunnel.clearDetail() } }
                    .safeAreaInset(edge: .bottom) {
                        VStack(spacing: 8) {
                            if tunnel.bodyVariant == nil {
                                Button(tunnel.detailRaw ? "查看摘要与请求头" : "查看原始详情") {
                                    tunnel.toggleDetailFormat()
                                }
                            } else {
                                Button("返回请求详情") { tunnel.showMetadataSummary() }
                            }
                            Button("复制到新请求") { tunnel.copySelectedRequest() }
                                .disabled(tunnel.replayLoading)
                            if tunnel.replayLoading { ProgressView("正在读取完整请求正文…") }
                            if let error = tunnel.replayError {
                                Text(error).font(.footnote).foregroundStyle(.red)
                            }
                            HStack {
                                if tunnel.hasRequestBody {
                                    Button("请求正文") { tunnel.showBodyPreview("request") }
                                }
                                if tunnel.hasResponseBody {
                                    Button("响应正文") { tunnel.showBodyPreview("response") }
                                }
                            }
                            if tunnel.bodyVariant != nil {
                                HStack {
                                    Button("上一段") { tunnel.previousBodyChunk() }
                                        .disabled(tunnel.bodyOffset == 0 || tunnel.bodyLoading)
                                    Text(tunnel.bodyPosition).font(.footnote).foregroundStyle(.secondary)
                                    Button("下一段") { tunnel.nextBodyChunk() }
                                        .disabled(tunnel.bodyNextOffset == nil || tunnel.bodyLoading)
                                }
                            }
                        }
                        .buttonStyle(.bordered)
                        .padding()
                        .frame(maxWidth: .infinity)
                        .background(.regularMaterial)
                    }
                }
                .sheet(item: $tunnel.replayDraft) { draft in
                    ManualRequestView(draft: draft).environmentObject(tunnel)
                }
            }
            .sheet(isPresented: $showingManualRequest) {
                ManualRequestView().environmentObject(tunnel)
            }
            .sheet(isPresented: $showingTrustedCertificates) {
                TrustedCertificatesView().environmentObject(tunnel)
            }
            .sheet(isPresented: $showingRules) {
                BlockingRulesView().environmentObject(tunnel)
            }
            .sheet(isPresented: $showingResponseRules) {
                ResponseRewriteRulesView().environmentObject(tunnel)
            }
        }
    }
}

private struct ResponseRewriteRulesView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var name = ""
    @State private var matchType = "host"
    @State private var pattern = ""
    @State private var method = ""
    @State private var modify = false
    @State private var status = "200"
    @State private var headers = ""
    @State private var responseBody = ""
    @State private var responseBodyBase64 = ""
    @State private var binaryResponseBody = false
    @State private var find = ""
    @State private var replacement = ""
    @State private var regex = false
    @State private var editingID: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("响应修改") {
                    Toggle("启用", isOn: Binding(get: { tunnel.responseEnabled },
                                              set: { tunnel.setResponseEnabled($0) }))
                    Picker("作用范围", selection: Binding(get: { tunnel.responseScope },
                                                       set: { tunnel.setResponseScope($0) })) {
                        Text("仅抓包").tag("capture")
                        Text("仅重放").tag("replay")
                        Text("抓包和重放").tag("all")
                    }
                }
                Section("已有规则") {
                    ForEach(tunnel.responseRules) { rule in
                        VStack(alignment: .leading) {
                            Toggle(isOn: Binding(get: { rule.enabled },
                                                 set: { tunnel.setResponseRuleEnabled(rule.id, $0) })) {
                                VStack(alignment: .leading) {
                                    Text(rule.name.isEmpty ? rule.pattern : rule.name)
                                    Text("\(rule.pattern) · \(rule.action == "modify" ? "局部修改" : "整份替换")")
                                        .font(.footnote).foregroundStyle(.secondary)
                                }
                            }
                            HStack {
                                Button("编辑") { loadEditor(rule) }
                                    .disabled(!rule.editable)
                                Spacer()
                                Button(role: .destructive) {
                                    if editingID == rule.id { resetEditor() }
                                    tunnel.removeResponseRule(rule.id)
                                } label: { Image(systemName: "trash") }
                            }
                            .buttonStyle(.borderless)
                        }
                    }
                }
                Section(editingID == nil ? "新增规则" : "编辑规则") {
                    TextField("规则名称（可选）", text: $name)
                    Picker("匹配方式", selection: $matchType) {
                        Text("精确域名").tag("host")
                        Text("URL 包含").tag("urlContains")
                        Text("URL 正则").tag("urlRegex")
                    }
                    TextField("域名或 URL 匹配内容", text: $pattern)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    TextField("方法（留空表示全部）", text: $method)
                        .textInputAutocapitalization(.characters)
                    Picker("操作", selection: $modify) {
                        Text("替换整份响应").tag(false)
                        Text("修改部分正文").tag(true)
                    }
                    if modify {
                        TextField("查找正文", text: $find)
                        TextField("替换为（可留空）", text: $replacement)
                        Toggle("正则表达式", isOn: $regex)
                    } else {
                        TextField("状态码，例如 200", text: $status).keyboardType(.numberPad)
                        Text("响应头，每行 名称: 值").font(.footnote).foregroundStyle(.secondary)
                        TextEditor(text: $headers).frame(minHeight: 100)
                        Toggle("按 Base64 输入二进制正文", isOn: $binaryResponseBody)
                        Text("两种格式使用独立输入框；保存时只采用当前显示的内容")
                            .font(.footnote).foregroundStyle(.secondary)
                        if binaryResponseBody {
                            Text("完整响应正文（Base64 原始字节）")
                                .font(.footnote).foregroundStyle(.secondary)
                            TextEditor(text: $responseBodyBase64).frame(minHeight: 150)
                                .textInputAutocapitalization(.never).autocorrectionDisabled()
                        } else {
                            Text("完整响应正文（UTF-8 文本）")
                                .font(.footnote).foregroundStyle(.secondary)
                            TextEditor(text: $responseBody).frame(minHeight: 150)
                        }
                    }
                    Button(editingID == nil ? "添加规则" : "保存修改") {
                        tunnel.addResponseRule(name: name, matchType: matchType,
                            pattern: pattern, method: method, modify: modify,
                            status: status, headers: headers, body: responseBody,
                            bodyBase64: binaryResponseBody ? responseBodyBase64 : nil,
                            find: find, replacement: replacement, regex: regex,
                            editingID: editingID)
                    }
                    if editingID != nil { Button("取消编辑") { resetEditor() } }
                }
                Text(tunnel.message).font(.footnote).foregroundStyle(.secondary)
            }
            .disabled(tunnel.responseSaving)
            .navigationTitle("响应修改规则")
            .toolbar { Button("关闭") { dismiss() } }
            .task { tunnel.refreshResponseRewrite() }
        }
    }

    private func loadEditor(_ rule: ResponseRule) {
        guard rule.editable else { return }
        editingID = rule.id
        name = rule.name
        matchType = rule.raw["matchType"] as? String ?? "host"
        pattern = rule.pattern
        method = rule.raw["method"] as? String ?? ""
        modify = rule.action == "modify"
        if modify {
            let change = (rule.raw["modifications"] as? [[String: Any]])?.first ?? [:]
            find = change["find"] as? String ?? ""
            replacement = change["value"] as? String ?? ""
            regex = change["regex"] as? Bool ?? false
        } else {
            let response = rule.raw["replacement"] as? [String: Any] ?? [:]
            status = String(response["status"] as? Int ?? 200)
            let rows = response["headers"] as? [[String: Any]] ?? []
            headers = rows.map { "\($0["name"] as? String ?? ""): \($0["value"] as? String ?? "")" }
                .joined(separator: "\n")
            let bytes = Data(base64Encoded: response["bodyBase64"] as? String ?? "") ?? Data()
            let readable = ResponseBodyCodec.readableText(bytes)
            responseBody = readable ?? ""
            responseBodyBase64 = response["bodyBase64"] as? String ?? ""
            binaryResponseBody = readable == nil
        }
    }

    private func resetEditor() {
        editingID = nil
        name = ""
        matchType = "host"
        pattern = ""
        method = ""
        modify = false
        status = "200"
        headers = ""
        responseBody = ""
        responseBodyBase64 = ""
        binaryResponseBody = false
        find = ""
        replacement = ""
        regex = false
    }
}

private struct ManualRequestView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var method = "GET"
    @State private var url = ""
    @State private var headers = ""
    @State private var requestBody = ""
    @State private var engine: ManualSendEngine = .auto
    @State private var bodyMode: ManualBodyMode = .text
    @State private var fileData: Data?
    @State private var fileName: String?
    @State private var fileMimeType: String?
    @State private var showingFileImporter = false
    @State private var inputError: String?

    private var headerLines: [String] { headers.components(separatedBy: "\n") }

    init(draft: ManualRequestDraft = .empty) {
        _method = State(initialValue: draft.method)
        _url = State(initialValue: draft.url)
        _headers = State(initialValue: draft.headers)
        _requestBody = State(initialValue: draft.body)
        _bodyMode = State(initialValue: draft.bodyMode)
        _engine = State(initialValue: draft.engine)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("请求") {
                    TextField("方法，例如 GET", text: $method)
                        .textInputAutocapitalization(.characters)
                    TextField("https://example.com/", text: $url)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("请求头，每行 名称: 值") {
                    TextEditor(text: $headers).frame(minHeight: 100)
                    Text("可粘贴浏览器请求头；请求行和 HTTP/2 伪头会忽略。行首 # 表示停用，发送时保留此状态。")
                        .font(.footnote).foregroundStyle(.secondary)
                    DisclosureGroup("逐项启用或停用") {
                        ForEach(headerLines.indices, id: \.self) { index in
                            let line = headerLines[index].trimmingCharacters(in: .whitespaces)
                            if RequestHeaderText.isHeaderLine(line) {
                                let disabled = line.hasPrefix("# ")
                                Button {
                                    headers = RequestHeaderText.toggle(headers, line: index)
                                } label: {
                                    Label(disabled ? String(line.dropFirst(2)) : line,
                                          systemImage: disabled ? "square" : "checkmark.square.fill")
                                        .lineLimit(1)
                                }
                            }
                        }
                    }
                }
                Section("发送协议") {
                    Picker("协议", selection: $engine) {
                        ForEach(ManualSendEngine.allCases) { option in
                            Text(option.label).tag(option)
                        }
                    }
                    Text("移动端使用原生 TLS；Chrome/Firefox 指纹发送引擎尚未内置。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("请求正文") {
                    Picker("类型", selection: $bodyMode) {
                        ForEach(ManualBodyMode.allCases) { mode in
                            Text(mode.rawValue).tag(mode)
                        }
                    }
                    if bodyMode != .file {
                        Text(bodyMode == .form || bodyMode == .multipart
                             ? "表单每行填写 名称=值" : bodyMode == .base64
                             ? "粘贴 Base64 编码的原始字节" : "输入正文")
                            .font(.footnote).foregroundStyle(.secondary)
                        TextEditor(text: $requestBody).frame(minHeight: 150)
                    }
                    if bodyMode == .file || bodyMode == .multipart {
                        Button("选择文件") { showingFileImporter = true }
                        if let fileName {
                            Text("已选择：\(fileName)（\(fileData?.count ?? 0) 字节）")
                                .font(.footnote).foregroundStyle(.secondary)
                            Button("移除文件") {
                                fileData = nil
                                self.fileName = nil
                                fileMimeType = nil
                            }
                        }
                    }
                    Text("文件与最终正文最多 1 MiB（Multipart 包含边界），文本输入最多 64 KiB。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                if tunnel.sendingManualRequest {
                    ProgressView("正在发送请求…")
                    if tunnel.canCancelManualUpload {
                        Button("取消正文上传", role: .cancel) { tunnel.cancelManualUpload() }
                    } else {
                        Text("请求正在处理；可返回首页等待结果。")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                }
                if let inputError { Text(inputError).foregroundStyle(.red) }
                Text(tunnel.message).font(.footnote).foregroundStyle(.secondary)
            }
            .navigationTitle("新建请求")
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button(tunnel.canCancelManualUpload ? "取消并关闭" :
                           tunnel.sendingManualRequest ? "后台等待" : "关闭") {
                        if tunnel.canCancelManualUpload { tunnel.cancelManualUpload() }
                        dismiss()
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("发送") {
                        do {
                            let prepared = try RequestBodyCodec.compose(
                                mode: bodyMode, text: requestBody, file: fileData,
                                fileName: fileName, mimeType: fileMimeType)
                            inputError = nil
                            tunnel.sendManualRequest(method: method, url: url, engine: engine,
                                                     headers: headers, body: prepared)
                        } catch {
                            inputError = error.localizedDescription
                        }
                    }.disabled(tunnel.sendingManualRequest)
                }
            }
            .fileImporter(isPresented: $showingFileImporter, allowedContentTypes: [.item]) { result in
                do {
                    let file = try result.get()
                    let access = file.startAccessingSecurityScopedResource()
                    defer { if access { file.stopAccessingSecurityScopedResource() } }
                    let handle = try FileHandle(forReadingFrom: file)
                    defer { try? handle.close() }
                    fileData = try RequestBodyCodec.readFile(handle)
                    fileName = file.lastPathComponent
                    fileMimeType = UTType(filenameExtension: file.pathExtension)?.preferredMIMEType
                    inputError = nil
                } catch {
                    inputError = error.localizedDescription
                }
            }
            .interactiveDismissDisabled(tunnel.sendingManualRequest)
            .onChange(of: tunnel.manualResultID) { resultID in
                if resultID != nil { dismiss() }
            }
        }
    }
}

private struct BlockingRulesView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var matchType = "host"
    @State private var pattern = ""
    @State private var method = ""
    @State private var editingID: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("请求阻止") {
                    Toggle("启用", isOn: Binding(get: { tunnel.blockingEnabled },
                                              set: { tunnel.setBlockingEnabled($0) }))
                    Picker("作用范围", selection: Binding(get: { tunnel.blockingScope },
                                                       set: { tunnel.setBlockingScope($0) })) {
                        Text("仅抓包").tag("capture")
                        Text("仅重放").tag("replay")
                        Text("抓包和重放").tag("all")
                    }
                    Text("匹配后返回 403，不向目标服务器发送 HTTP 请求。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("规则") {
                    ForEach(tunnel.blockingRules) { rule in
                        VStack(alignment: .leading) {
                            Toggle(isOn: Binding(get: { rule.enabled },
                                                 set: { tunnel.setRuleEnabled(rule.id, $0) })) {
                                VStack(alignment: .leading) {
                                    Text(rule.pattern)
                                    Text("\(rule.matchType) · \(rule.method.isEmpty ? "全部方法" : rule.method)")
                                        .font(.footnote).foregroundStyle(.secondary)
                                }
                            }
                            HStack {
                                Button("编辑") {
                                    editingID = rule.id
                                    matchType = rule.matchType
                                    pattern = rule.pattern
                                    method = rule.method
                                }
                                Spacer()
                                Button(role: .destructive) {
                                    if editingID == rule.id { resetEditor() }
                                    tunnel.removeRule(rule.id)
                                } label: { Image(systemName: "trash") }
                            }
                            .buttonStyle(.borderless)
                        }
                    }
                }
                Section(editingID == nil ? "新增规则" : "编辑规则") {
                    Picker("匹配方式", selection: $matchType) {
                        Text("精确域名").tag("host")
                        Text("URL 包含").tag("urlContains")
                        Text("URL 正则").tag("urlRegex")
                    }
                    TextField("域名或 URL 匹配内容", text: $pattern)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    TextField("方法（留空表示全部）", text: $method)
                        .textInputAutocapitalization(.characters)
                    Button(editingID == nil ? "添加规则" : "保存修改") {
                        let normalized = method.trimmingCharacters(in: .whitespaces).uppercased()
                        if let editingID {
                            tunnel.editBlockingRule(id: editingID, matchType: matchType,
                                                    pattern: pattern, method: normalized)
                        } else {
                            tunnel.addBlockingRule(matchType: matchType, pattern: pattern,
                                                   method: normalized)
                        }
                    }
                    Button(editingID == nil ? "清空表单" : "取消编辑") { resetEditor() }
                }
                if tunnel.blockingSaving { ProgressView("正在保存阻止规则…") }
                Text(tunnel.message).font(.footnote).foregroundStyle(.secondary)
            }
            .disabled(tunnel.blockingSaving)
            .navigationTitle("阻止规则")
            .toolbar { Button("关闭") { dismiss() } }
            .task { tunnel.refreshBlocking() }
        }
    }

    private func resetEditor() {
        editingID = nil
        matchType = "host"
        pattern = ""
        method = ""
    }
}
