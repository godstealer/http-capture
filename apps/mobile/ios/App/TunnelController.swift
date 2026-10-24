import Foundation
import NetworkExtension
import Combine

struct CapturedRequest: Identifiable {
    let id: String
    let method: String
    let url: String
    let status: Int?

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String else { return nil }
        self.id = id
        method = value["method"] as? String ?? "?"
        url = value["url"] as? String ?? ""
        status = value["status"] as? Int
    }
}

struct BlockingRule: Identifiable {
    var id: String
    var enabled: Bool
    var matchType: String
    var pattern: String
    var method: String

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let matchType = value["matchType"] as? String,
              let pattern = value["pattern"] as? String else { return nil }
        self.id = id
        self.enabled = value["enabled"] as? Bool ?? false
        self.matchType = matchType
        self.pattern = pattern
        self.method = value["method"] as? String ?? ""
    }

    init(matchType: String, pattern: String, method: String) {
        id = UUID().uuidString
        enabled = true
        self.matchType = matchType
        self.pattern = pattern
        self.method = method
    }

    var dictionary: [String: Any] {
        ["id": id, "enabled": enabled, "matchType": matchType,
         "pattern": pattern, "method": method]
    }
}

struct ResponseRule: Identifiable {
    let id: String
    let name: String
    let pattern: String
    let action: String
    let enabled: Bool
    let raw: [String: Any]

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String,
              let pattern = value["pattern"] as? String,
              let action = value["action"] as? String else { return nil }
        self.id = id
        self.name = value["name"] as? String ?? ""
        self.pattern = pattern
        self.action = action
        self.enabled = value["enabled"] as? Bool ?? false
        self.raw = value
    }

    var editable: Bool {
        guard let matchType = raw["matchType"] as? String,
              ["host", "urlContains", "urlRegex"].contains(matchType) else { return false }
        if action == "modify" {
            guard raw["replacement"] == nil || raw["replacement"] is NSNull,
                  let changes = raw["modifications"] as? [[String: Any]], changes.count == 1,
                  let change = changes.first,
                  change["target"] as? String == "body",
                  change["operation"] as? String == "replace",
                  let find = change["find"] as? String,
                  let value = change["value"] as? String else { return false }
            return find.utf8.count <= 64 * 1024 && value.utf8.count <= 64 * 1024
        }
        guard action == "replace",
              (raw["modifications"] as? [Any] ?? []).isEmpty,
              let response = raw["replacement"] as? [String: Any],
              let base64 = response["bodyBase64"] as? String,
              base64.utf8.count <= 96 * 1024,
              let body = Data(base64Encoded: base64), body.count <= 64 * 1024,
              String(data: body, encoding: .utf8) != nil,
              let headers = response["headers"] as? [[String: Any]] else { return false }
        var length = 0
        for header in headers {
            guard header["disabled"] as? Bool != true,
                  let name = header["name"] as? String,
                  let value = header["value"] as? String,
                  value == value.trimmingCharacters(in: .whitespaces) else { return false }
            length += name.utf8.count + value.utf8.count + 4
        }
        return length <= 16 * 1024
    }
}

@MainActor
final class TunnelController: ObservableObject {
    @Published private(set) var connected = false
    @Published private(set) var tunnelStarting = false
    @Published private(set) var tunnelStopping = false
    @Published private(set) var busy = false
    @Published private(set) var message = "VPN 未启动"
    @Published private(set) var requests: [CapturedRequest] = []
    @Published private(set) var selectedDetail = ""
    @Published private(set) var detailRaw = false
    @Published private(set) var detailOpen = false
    @Published private(set) var selectedFlow: [String: Any]?
    @Published private(set) var bodyVariant: String?
    @Published private(set) var bodyOffset = 0
    @Published private(set) var bodyNextOffset: Int?
    @Published private(set) var bodyTotalBytes = 0
    @Published private(set) var bodyLoading = false
    @Published private(set) var replayLoading = false
    @Published private(set) var replayError: String?
    @Published var replayDraft: ManualRequestDraft?
    @Published private(set) var certificateURL: URL?
    @Published private(set) var blockingEnabled = false
    @Published private(set) var blockingScope = "capture"
    @Published private(set) var blockingRules: [BlockingRule] = []
    @Published private(set) var blockingSaving = false
    @Published private(set) var responseEnabled = false
    @Published private(set) var responseScope = "capture"
    @Published private(set) var responseRules: [ResponseRule] = []
    @Published private(set) var responseSaving = false
    @Published private(set) var sendingManualRequest = false
    @Published var port = 8899
    @Published var urlFilterInput = ""

    private var manager: NETunnelProviderManager?
    private var cursor: String?
    private var urlFilter = ""
    private var requestGeneration: UInt64 = 0
    private var detailGeneration: UInt64 = 0
    private var replayGeneration: UInt64 = 0
    private var bodyOffsetHistory: [Int] = []
    private var responseConfig: [String: Any] = [:]
    private var responseVersion = ""
    private var rawDetail = ""
    private var statusObserver: NSObjectProtocol?

    var shouldOfferStop: Bool { connected || tunnelStarting }

    init() { loadProfile() }

    deinit {
        if let statusObserver { NotificationCenter.default.removeObserver(statusObserver) }
    }

    func loadProfile() {
        NETunnelProviderManager.loadAllFromPreferences { [weak self] managers, error in
            Task { @MainActor [weak self] in
                guard let self else { return }
                if let error { self.message = error.localizedDescription; return }
                self.manager = managers?.first(where: {
                    ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == "dev.httpcapture.mobile.ios.PacketTunnel"
                })
                if let number = (self.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["port"] as? NSNumber {
                    self.port = number.intValue
                }
                self.watchStatus()
                self.refreshStatus()
            }
        }
    }

    func start() {
        if busy || shouldOfferStop || tunnelStopping { return }
        guard (1...65535).contains(port) else {
            message = "端口必须在 1 到 65535 之间"
            return
        }
        busy = true
        let profile = manager ?? NETunnelProviderManager()
        let configuration = NETunnelProviderProtocol()
        configuration.providerBundleIdentifier = "dev.httpcapture.mobile.ios.PacketTunnel"
        configuration.serverAddress = "HTTP Capture"
        configuration.providerConfiguration = ["port": port]
        profile.protocolConfiguration = configuration
        profile.localizedDescription = "HTTP Capture"
        profile.isEnabled = true
        profile.saveToPreferences { [weak self] error in
            guard let self else { return }
            if let error {
                Task { @MainActor in self.busy = false; self.message = error.localizedDescription }
                return
            }
            profile.loadFromPreferences { [weak self] error in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    self.busy = false
                    if let error { self.message = error.localizedDescription; return }
                    self.manager = profile
                    self.watchStatus()
                    do {
                        try profile.connection.startVPNTunnel()
                        self.tunnelStarting = true
                        self.tunnelStopping = false
                        self.message = "正在建立 VPN…"
                    } catch {
                        self.message = error.localizedDescription
                    }
                }
            }
        }
    }

    func stop() {
        guard shouldOfferStop, !tunnelStopping else { return }
        tunnelStarting = false
        tunnelStopping = true
        manager?.connection.stopVPNTunnel()
        message = "正在停止 VPN…"
    }

    func refreshRequests(loadMore: Bool = false) {
        var command: [String: Any] = ["method": "recent"]
        if !urlFilter.isEmpty { command["urlContains"] = urlFilter }
        if loadMore {
            guard let cursor, let value = Int64(cursor) else { return }
            command["cursor"] = value
        } else {
            cursor = nil
        }
        requestGeneration &+= 1
        let generation = requestGeneration
        send(command) { [weak self] result in
            guard let self, let page = result as? [String: Any] else { return }
            guard self.requestGeneration == generation else { return }
            let incoming = ((page["requests"] as? [[String: Any]]) ?? []).compactMap(CapturedRequest.init)
            self.requests = loadMore ? self.requests + incoming : incoming
            self.cursor = page["nextCursor"] as? String
        }
    }

    var hasMoreRequests: Bool { cursor != nil }

    func applyUrlFilter() {
        let filter = urlFilterInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard filter.utf8.count <= 2048 else {
            message = "URL 搜索内容不能超过 2048 字节"
            return
        }
        urlFilter = filter
        cursor = nil
        requests = []
        refreshRequests()
    }

    func showDetail(id: String) {
        detailGeneration &+= 1
        let generation = detailGeneration
        send(["method": "metadata", "id": id]) { [weak self] value in
            guard let self, self.detailGeneration == generation,
                  let flow = value as? [String: Any],
                  let data = try? JSONSerialization.data(withJSONObject: flow, options: [.prettyPrinted, .sortedKeys]) else { return }
            self.selectedFlow = flow
            self.detailOpen = true
            self.bodyVariant = nil
            self.bodyOffsetHistory = []
            self.bodyNextOffset = nil
            self.bodyLoading = false
            self.rawDetail = String(decoding: data, as: UTF8.self)
            self.detailRaw = false
            self.selectedDetail = FlowDetailFormatter.format(flow)
        }
    }

    func toggleDetailFormat() {
        guard let flow = selectedFlow, bodyVariant == nil else { return }
        detailRaw.toggle()
        selectedDetail = detailRaw ? rawDetail : FlowDetailFormatter.format(flow)
    }

    func showMetadataSummary() {
        guard let flow = selectedFlow else { return }
        detailGeneration &+= 1
        bodyVariant = nil
        bodyOffsetHistory = []
        bodyNextOffset = nil
        bodyLoading = false
        selectedDetail = detailRaw ? rawDetail : FlowDetailFormatter.format(flow)
    }

    var hasRequestBody: Bool { (selectedFlow?["bodyReferences"] as? [String: Any])?["request"] != nil }
    var hasResponseBody: Bool { (selectedFlow?["bodyReferences"] as? [String: Any])?["response"] != nil }

    var bodyPosition: String {
        guard bodyVariant != nil else { return "" }
        if bodyTotalBytes == 0 { return "0 / 0 字节" }
        return "\(bodyOffset + 1)–\(min(bodyTotalBytes, bodyNextOffset ?? bodyTotalBytes)) / \(bodyTotalBytes) 字节"
    }

    func showBodyPreview(_ variant: String, offset: Int = 0, history: [Int] = []) {
        guard let flow = selectedFlow,
              let id = flow["id"] as? String,
              let revision = flow["bodyRevision"] as? String,
              offset >= 0 else { return }
        detailGeneration &+= 1
        let generation = detailGeneration
        bodyLoading = true
        send(["method": "body_preview", "id": id, "variant": variant,
              "revision": revision, "offset": offset], onFailure: { [weak self] in
            guard let self, self.detailGeneration == generation else { return }
            self.bodyLoading = false
        }) { [weak self] value in
            guard let self, self.detailGeneration == generation else { return }
            self.bodyLoading = false
            guard let chunk = value as? [String: Any],
                  let currentOffset = (chunk["offset"] as? NSNumber)?.intValue,
                  let totalBytes = (chunk["totalBytes"] as? NSNumber)?.intValue else { return }
            let text = chunk["text"] as? String
                ?? "二进制内容（Base64）：\n" + (chunk["bodyBase64"] as? String ?? "")
            self.bodyVariant = variant
            self.bodyOffsetHistory = history
            self.bodyOffset = currentOffset
            self.bodyTotalBytes = totalBytes
            self.bodyNextOffset = (chunk["nextOffset"] as? NSNumber)?.intValue
            self.selectedDetail = text
        }
    }

    func previousBodyChunk() {
        guard let variant = bodyVariant, let previous = bodyOffsetHistory.last, !bodyLoading else { return }
        showBodyPreview(variant, offset: previous, history: Array(bodyOffsetHistory.dropLast()))
    }

    func nextBodyChunk() {
        guard let variant = bodyVariant, let offset = bodyNextOffset, !bodyLoading else { return }
        showBodyPreview(variant, offset: offset, history: bodyOffsetHistory + [bodyOffset])
    }

    func copySelectedRequest() {
        guard let selectedFlow, !replayLoading else { return }
        replayError = nil
        do {
            let copy = try RecordedRequestCopy(flow: selectedFlow)
            replayGeneration &+= 1
            replayLoading = true
            readCopyChunk(copy, generation: replayGeneration)
        } catch {
            replayError = error.localizedDescription
        }
    }

    private func readCopyChunk(_ copy: RecordedRequestCopy, generation: UInt64) {
        guard replayGeneration == generation else { return }
        if copy.offset == copy.byteLength {
            do {
                replayDraft = try copy.draft()
                replayLoading = false
            } catch {
                replayError = error.localizedDescription
                replayLoading = false
            }
            return
        }
        send(["method": "body_preview", "id": copy.id, "variant": "request",
              "revision": copy.revision, "offset": copy.offset], onFailure: { [weak self] in
            guard let self, self.replayGeneration == generation else { return }
            self.replayLoading = false
            self.replayError = self.message
        }) { [weak self] value in
            guard let self, self.replayGeneration == generation else { return }
            do {
                guard let chunk = value as? [String: Any] else {
                    throw RecordedRequestCopy.CopyError.invalid("请求正文读取失败")
                }
                var next = copy
                _ = try next.append(chunk)
                self.readCopyChunk(next, generation: generation)
            } catch {
                self.replayError = error.localizedDescription
                self.replayLoading = false
            }
        }
    }

    func clearDetail() {
        detailGeneration &+= 1
        replayGeneration &+= 1
        selectedDetail = ""
        rawDetail = ""
        detailRaw = false
        detailOpen = false
        selectedFlow = nil
        bodyVariant = nil
        bodyOffsetHistory = []
        bodyNextOffset = nil
        bodyLoading = false
        replayLoading = false
        replayError = nil
        replayDraft = nil
    }

    func refreshBlocking() {
        send(["method": "blocking"]) { [weak self] value in
            guard let self, let config = value as? [String: Any] else { return }
            self.blockingEnabled = config["enabled"] as? Bool ?? false
            self.blockingScope = config["scope"] as? String ?? "capture"
            self.blockingRules = ((config["rules"] as? [[String: Any]]) ?? []).compactMap(BlockingRule.init)
        }
    }

    func setBlockingEnabled(_ enabled: Bool) { saveBlocking(enabled: enabled, rules: blockingRules) }

    func setBlockingScope(_ scope: String) {
        guard ["capture", "replay", "all"].contains(scope) else { return }
        saveBlocking(enabled: blockingEnabled, rules: blockingRules, scope: scope)
    }

    func addBlockingRule(matchType: String, pattern: String, method: String) {
        let pattern = pattern.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !pattern.isEmpty else { message = "请填写匹配内容"; return }
        saveBlocking(enabled: true,
                     rules: blockingRules + [BlockingRule(matchType: matchType, pattern: pattern, method: method)])
    }

    func editBlockingRule(id: String, matchType: String, pattern: String, method: String) {
        let pattern = pattern.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !pattern.isEmpty else { message = "请填写匹配内容"; return }
        guard ["host", "urlContains", "urlRegex"].contains(matchType) else { return }
        var rules = blockingRules
        guard let index = rules.firstIndex(where: { $0.id == id }) else {
            message = "规则已变化，请刷新后重试"
            return
        }
        rules[index].matchType = matchType
        rules[index].pattern = pattern
        rules[index].method = method
        saveBlocking(enabled: blockingEnabled, rules: rules)
    }

    func setRuleEnabled(_ id: String, _ enabled: Bool) {
        saveBlocking(enabled: blockingEnabled,
                     rules: blockingRules.map { rule in
                         var copy = rule
                         if copy.id == id { copy.enabled = enabled }
                         return copy
                     })
    }

    func removeRule(_ id: String) {
        saveBlocking(enabled: blockingEnabled, rules: blockingRules.filter { $0.id != id })
    }

    private func saveBlocking(enabled: Bool, rules: [BlockingRule], scope: String? = nil) {
        guard !blockingSaving else { return }
        let config: [String: Any] = ["enabled": enabled, "scope": scope ?? blockingScope,
                                     "rules": rules.map(\.dictionary)]
        blockingSaving = true
        send(["method": "configure_blocking", "config": config], onFailure: { [weak self] in
            self?.blockingSaving = false
            self?.refreshBlocking()
        }) { [weak self] value in
            guard let self else { return }
            self.blockingSaving = false
            guard let saved = value as? [String: Any] else {
                self.message = "VPN 扩展没有返回阻止规则配置"
                return
            }
            self.blockingEnabled = saved["enabled"] as? Bool ?? false
            self.blockingScope = saved["scope"] as? String ?? "capture"
            self.blockingRules = ((saved["rules"] as? [[String: Any]]) ?? []).compactMap(BlockingRule.init)
            self.message = "阻止规则已保存"
        }
    }

    func refreshResponseRewrite() {
        send(["method": "response_rewrite"]) { [weak self] value in
            guard let self, let snapshot = value as? [String: Any] else { return }
            self.applyResponseSnapshot(snapshot)
        }
    }

    private func applyResponseSnapshot(_ snapshot: [String: Any]) {
        guard let config = snapshot["config"] as? [String: Any],
              let version = snapshot["version"] as? String else { return }
        responseConfig = config
        responseVersion = version
        responseEnabled = config["enabled"] as? Bool ?? false
        responseScope = config["scope"] as? String ?? "capture"
        responseRules = ((config["rules"] as? [[String: Any]]) ?? []).compactMap(ResponseRule.init)
    }

    func setResponseEnabled(_ enabled: Bool) {
        var config = responseConfig
        config["enabled"] = enabled
        saveResponseRewrite(config)
    }

    func setResponseScope(_ scope: String) {
        var config = responseConfig
        config["scope"] = scope
        saveResponseRewrite(config)
    }

    func setResponseRuleEnabled(_ id: String, _ enabled: Bool) {
        var config = responseConfig
        var rules = config["rules"] as? [[String: Any]] ?? []
        guard let index = rules.firstIndex(where: { $0["id"] as? String == id }) else { return }
        rules[index]["enabled"] = enabled
        config["rules"] = rules
        saveResponseRewrite(config)
    }

    func removeResponseRule(_ id: String) {
        var config = responseConfig
        let rules = config["rules"] as? [[String: Any]] ?? []
        config["rules"] = rules.filter { $0["id"] as? String != id }
        saveResponseRewrite(config)
    }

    func addResponseRule(name: String, matchType: String, pattern: String, method: String,
                         modify: Bool, status: String, headers: String, body: String,
                         find: String, replacement: String, regex: Bool,
                         editingID: String? = nil) {
        let pattern = pattern.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !pattern.isEmpty else { message = "请填写匹配内容"; return }
        var config = responseConfig
        var rules = config["rules"] as? [[String: Any]] ?? []
        let editingIndex = editingID.flatMap { id in
            rules.firstIndex(where: { $0["id"] as? String == id })
        }
        if editingID != nil {
            guard let index = editingIndex, ResponseRule(rules[index])?.editable == true else {
                message = "这条规则无法在手机编辑，请重新加载"
                return
            }
        } else if rules.count >= 32 {
            message = "最多支持 32 条响应规则"
            return
        }
        var rule: [String: Any] = editingIndex.map { rules[$0] } ?? [
            "id": "ios-" + UUID().uuidString, "enabled": true
        ]
        rule["name"] = name.trimmingCharacters(in: .whitespacesAndNewlines)
        rule["matchType"] = matchType
        rule["pattern"] = pattern
        rule["method"] = method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        if modify {
            guard !find.isEmpty, find.utf8.count <= 64 * 1024,
                  replacement.utf8.count <= 64 * 1024 else {
                message = "查找内容不能为空，查找和替换文本各最多 64 KiB"
                return
            }
            var change = (rule["action"] as? String == "modify"
                ? (rule["modifications"] as? [[String: Any]])?.first : nil) ?? [:]
            change["target"] = "body"
            change["operation"] = "replace"
            change["find"] = find
            change["value"] = replacement
            change["regex"] = regex
            rule["action"] = "modify"
            rule["replacement"] = nil
            rule["modifications"] = [change]
        } else {
            guard let code = UInt16(status.trimmingCharacters(in: .whitespacesAndNewlines)),
                  (200...599).contains(code), body.utf8.count <= 64 * 1024,
                  headers.utf8.count <= 16 * 1024 else {
                message = "状态码需为 200–599，正文最多 64 KiB，响应头最多 16 KiB"
                return
            }
            var rows: [[String: String]] = []
            for line in headers.components(separatedBy: .newlines)
                where !line.trimmingCharacters(in: .whitespaces).isEmpty {
                guard let colon = line.firstIndex(of: ":"), colon != line.startIndex else {
                    message = "响应头需要按 名称: 值 填写"
                    return
                }
                rows.append(["name": String(line[..<colon]).trimmingCharacters(in: .whitespaces),
                             "value": String(line[line.index(after: colon)...]).trimmingCharacters(in: .whitespaces)])
            }
            rule["action"] = "replace"
            rule["modifications"] = nil
            rule["replacement"] = ["status": Int(code), "headers": rows,
                                   "bodyBase64": Data(body.utf8).base64EncodedString()]
        }
        if let index = editingIndex { rules[index] = rule }
        else { rules.append(rule) }
        config["rules"] = rules
        if editingIndex == nil { config["enabled"] = true }
        saveResponseRewrite(config)
    }

    private func saveResponseRewrite(_ config: [String: Any]) {
        guard !responseSaving, !responseVersion.isEmpty else { return }
        let command: [String: Any] = ["method": "configure_response_rewrite",
                                      "expectedVersion": responseVersion, "config": config]
        guard let data = try? JSONSerialization.data(withJSONObject: command),
              data.count <= 128 * 1024 else {
            message = "响应规则配置超过扩展消息上限 128 KiB"
            return
        }
        responseSaving = true
        send(command, onFailure: { [weak self] in
            self?.responseSaving = false
            self?.refreshResponseRewrite()
        }) { [weak self] value in
            guard let self else { return }
            self.responseSaving = false
            if let snapshot = value as? [String: Any] {
                self.applyResponseSnapshot(snapshot)
                self.message = "响应修改规则已保存"
            }
        }
    }

    func sendManualRequest(method: String, url: String, engine: ManualSendEngine,
                           headers: String, body: PreparedRequestBody) {
        guard !sendingManualRequest else { return }
        let method = method.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        let url = url.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !method.isEmpty, let parsed = URLComponents(string: url),
              ["http", "https"].contains(parsed.scheme?.lowercased() ?? ""), parsed.host != nil else {
            message = "请输入 HTTP(S) URL 和请求方法"
            return
        }
        if engine == .h3 && parsed.scheme?.lowercased() != "https" {
            message = "HTTP/3 需要 HTTPS URL"
            return
        }
        guard body.bytes.count <= RequestBodyCodec.maxBodyBytes, headers.utf8.count <= 16 * 1024 else {
            message = "正文最多 1 MiB，请求头最多 16 KiB"
            return
        }
        var headerRows: [[String: Any]] = []
        for line in headers.components(separatedBy: .newlines) where !line.trimmingCharacters(in: .whitespaces).isEmpty {
            guard let colon = line.firstIndex(of: ":"), colon != line.startIndex else {
                message = "请求头需要按 名称: 值 填写"
                return
            }
            headerRows.append(["name": String(line[..<colon]).trimmingCharacters(in: .whitespaces),
                               "value": String(line[line.index(after: colon)...]).trimmingCharacters(in: .whitespaces)])
        }
        if let contentType = body.contentType {
            if body.replaceContentType {
                headerRows.removeAll {
                    ($0["name"] as? String)?.lowercased() == "content-type"
                }
            }
            let hasContentType = headerRows.contains {
                ($0["name"] as? String)?.lowercased() == "content-type"
            }
            if !hasContentType {
                headerRows.append(["name": "Content-Type", "value": contentType])
            }
        }
        var draft: [String: Any] = ["method": method, "url": url, "engine": engine.rawValue,
                                    "headers": headerRows, "bodyBase64": ""]
        sendingManualRequest = true
        message = "正在发送请求…"
        if body.bytes.count <= 48 * 1024 {
            draft["bodyBase64"] = body.bytes.base64EncodedString()
            let direct: [String: Any] = ["method": "send_request", "request": draft]
            if let data = try? JSONSerialization.data(withJSONObject: direct), data.count <= 128 * 1024 {
                send(direct, onFailure: { [weak self] in self?.sendingManualRequest = false }) {
                    [weak self] value in self?.finishManualRequest(value)
                }
                return
            }
            draft["bodyBase64"] = ""
        }
        let begin: [String: Any] = ["method": "begin_request_upload", "request": draft,
                                     "totalBytes": body.bytes.count]
        guard let data = try? JSONSerialization.data(withJSONObject: begin), data.count <= 128 * 1024 else {
            sendingManualRequest = false
            message = "请求头超过 VPN 扩展消息上限 128 KiB"
            return
        }
        send(begin, onFailure: { [weak self] in self?.sendingManualRequest = false }) { [weak self] value in
            guard let self else { return }
            guard let reply = value as? [String: Any],
                  let token = (reply["token"] as? NSNumber)?.uint64Value,
                  (reply["nextOffset"] as? NSNumber)?.intValue == 0 else {
                self.sendingManualRequest = false
                self.message = "VPN 扩展没有创建请求上传会话"
                return
            }
            self.appendManualBody(body.bytes, token: token, offset: 0)
        }
    }

    private func appendManualBody(_ body: Data, token: UInt64, offset: Int) {
        guard sendingManualRequest else { return }
        if offset == body.count {
            send(["method": "commit_request_upload", "token": token],
                 onFailure: { [weak self] in self?.sendingManualRequest = false }) {
                [weak self] value in self?.finishManualRequest(value)
            }
            return
        }
        let end = min(offset + 16 * 1024, body.count)
        let command: [String: Any] = ["method": "append_request_upload", "token": token,
                                      "offset": offset,
                                      "bodyBase64": body.subdata(in: offset..<end).base64EncodedString()]
        send(command, onFailure: { [weak self] in self?.sendingManualRequest = false }) { [weak self] value in
            guard let self else { return }
            guard let result = value as? [String: Any],
                  (result["nextOffset"] as? NSNumber)?.intValue == end else {
                self.sendingManualRequest = false
                self.message = "请求正文上传位置不一致，请重试"
                self.send(["method": "cancel_request_upload", "token": token]) { _ in }
                return
            }
            self.appendManualBody(body, token: token, offset: end)
        }
    }

    private func finishManualRequest(_ value: Any?) {
        sendingManualRequest = false
        guard let result = value as? [String: Any] else {
            message = "VPN 扩展没有返回发送结果"
            return
        }
        if let error = result["error"] as? String { message = "请求失败：" + error }
        else { message = "请求已发送，状态 \(result["status"] ?? "未知")" }
        refreshRequests()
        if let id = result["id"] as? String { showDetail(id: id) }
    }

    func exportCertificate() {
        send(["method": "certificate"]) { [weak self] value in
            guard let self, let pem = value as? String else { return }
            do {
                let file = FileManager.default.temporaryDirectory.appendingPathComponent("http-capture-ca.pem")
                try pem.write(to: file, atomically: true, encoding: .utf8)
                self.certificateURL = file
            } catch {
                self.message = error.localizedDescription
            }
        }
    }

    private func send(_ command: [String: Any], onFailure: (() -> Void)? = nil,
                      completion: @escaping (Any?) -> Void) {
        guard let session = manager?.connection as? NETunnelProviderSession,
              let data = try? JSONSerialization.data(withJSONObject: command) else {
            message = "请先启动 VPN"
            onFailure?()
            return
        }
        do {
            try session.sendProviderMessage(data) { [weak self] response in
                Task { @MainActor [weak self] in
                    guard let self else { return }
                    guard let response,
                          let object = (try? JSONSerialization.jsonObject(with: response)) as? [String: Any] else {
                        self.message = "VPN 扩展没有返回结果"
                        onFailure?()
                        return
                    }
                    if object["ok"] as? Bool == true { completion(object["value"]) }
                    else {
                        self.message = object["error"] as? String ?? "VPN 请求失败"
                        onFailure?()
                    }
                }
            }
        } catch {
            message = error.localizedDescription
            onFailure?()
        }
    }

    private func watchStatus() {
        if let statusObserver { NotificationCenter.default.removeObserver(statusObserver) }
        guard let manager else { return }
        statusObserver = NotificationCenter.default.addObserver(forName: .NEVPNStatusDidChange,
                                                                 object: manager.connection, queue: .main) { [weak self] _ in
            Task { @MainActor [weak self] in self?.refreshStatus() }
        }
    }

    private func refreshStatus() {
        switch manager?.connection.status {
        case .connected:
            if tunnelStopping {
                manager?.connection.stopVPNTunnel()
                connected = false
                message = "正在停止 VPN…"
                return
            }
            connected = true
            tunnelStarting = false
            tunnelStopping = false
            message = "VPN 抓包运行中"
            refreshRequests()
        case .connecting, .reasserting:
            if tunnelStopping {
                connected = false
                message = "正在停止 VPN…"
                return
            }
            connected = false
            tunnelStarting = true
            tunnelStopping = false
            requestGeneration &+= 1
            cursor = nil
            message = "正在建立 VPN…"
        case .disconnecting:
            connected = false
            tunnelStarting = false
            tunnelStopping = true
            requestGeneration &+= 1
            cursor = nil
            message = "正在停止 VPN…"
        default:
            connected = false
            tunnelStarting = false
            tunnelStopping = false
            requestGeneration &+= 1
            cursor = nil
            message = "VPN 未启动"
        }
    }
}
