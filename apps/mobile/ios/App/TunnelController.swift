import Foundation
import NetworkExtension
import Combine

struct CapturedRequest: Identifiable {
    let id: String
    let method: String
    let url: String
    let status: Int?
    let sourceLabel: String?

    init?(_ value: [String: Any]) {
        guard let id = value["id"] as? String else { return nil }
        self.id = id
        method = value["method"] as? String ?? "?"
        url = value["url"] as? String ?? ""
        status = value["status"] as? Int
        switch (value["source"] as? String) ?? "" {
        case "capture": sourceLabel = "抓包"
        case "replay": sourceLabel = "手动"
        default: sourceLabel = nil
        }
    }
}

struct TrustedCertificate: Identifiable {
    let id: String
    let name: String
    let host: String
    let port: Int
    let enabled: Bool

    init?(_ value: [String: Any]) {
        guard value["kind"] as? String == "trust", let id = value["id"] as? String else { return nil }
        self.id = id
        name = value["name"] as? String ?? "CA"
        host = value["host"] as? String ?? ""
        port = value["port"] as? Int ?? 443
        enabled = value["enabled"] as? Bool ?? false
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
              (try? ResponseBodyCodec.decodeBase64(base64)) != nil,
              let headers = response["headers"] as? [[String: Any]] else { return false }
        var length = 0
        for header in headers {
            guard let name = header["name"] as? String,
                  let value = header["value"] as? String,
                  value == value.trimmingCharacters(in: .whitespaces) else { return false }
            let disabled = header["disabled"] as? Bool == true
            let rendered = (disabled ? "# " : "") + name + ": " + value
            guard let roundtrip = try? RequestHeaderText.parseResponse(rendered),
                  roundtrip.count == 1,
                  roundtrip[0]["name"] as? String == name,
                  roundtrip[0]["value"] as? String == value,
                  roundtrip[0]["disabled"] as? Bool == disabled else { return false }
            length += rendered.utf8.count + 1
        }
        return length <= 16 * 1024
    }
}

@MainActor
final class TunnelController: ObservableObject {
    @Published private(set) var connected = false
    @Published private(set) var tunnelStarting = false
    @Published private(set) var tunnelStopping = false
    @Published private(set) var busy = true
    @Published private(set) var profileLoaded = false
    @Published private(set) var configurationBusy = false
    @Published private(set) var configurationRequiresRestart = false
    @Published private(set) var message = "正在读取 VPN 配置…"
    @Published private(set) var requests: [CapturedRequest] = []
    @Published private(set) var requestPageLoading = false
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
    @Published private(set) var sessionExportURL: URL?
    @Published private(set) var sessionExporting = false
    @Published private(set) var sessionExportCancelPending = false
    @Published private(set) var sessionExportProgress = 0
    private var sessionExportGeneration: UInt64 = 0
    private var sessionExportFile: FileHandle?
    private var sessionExportToken: UInt64?
    private var sessionExportTempURL: URL?
    private var sessionExportOffline = false
    @Published private(set) var trustedCertificates: [TrustedCertificate] = []
    @Published private(set) var trustedCertificatesBusy = false
    private var trustedCertificatesVersion = ""
    @Published private(set) var blockingEnabled = false
    @Published private(set) var blockingScope = "capture"
    @Published private(set) var blockingRules: [BlockingRule] = []
    @Published private(set) var blockingSaving = false
    @Published private(set) var responseEnabled = false
    @Published private(set) var responseScope = "capture"
    @Published private(set) var responseRules: [ResponseRule] = []
    @Published private(set) var responseSaving = false
    @Published private(set) var sendingManualRequest = false
    @Published private(set) var canCancelManualUpload = false
    @Published private(set) var manualResultID: String?
    @Published private(set) var manualSaveCounter: UInt64 = 0
    @Published private(set) var savedRequests: [SavedRequestSummary] = []
    @Published private(set) var savedCollections: [SavedCollectionSummary] = []
    @Published private(set) var savedRequestTotal = 0
    @Published private(set) var savedRequestQuery = ""
    @Published private(set) var savedCollectionID = ""
    @Published private(set) var savedRequestOffset = 0
    @Published private(set) var savedRequestNextOffset: Int?
    @Published private(set) var savedRequestLoading = false
    @Published private(set) var savedCollectionLoading = false
    @Published private(set) var savedCollectionSaving = false
    @Published private(set) var savedOpenLoading = false
    @Published private(set) var savedDeleteBusy = false
    @Published private(set) var savedRequestError: String?
    @Published var savedDraft: ManualRequestDraft?
    @Published var port = UserDefaults.standard.object(forKey: "capture_port") as? Int ?? 8899 {
        didSet { UserDefaults.standard.set(port, forKey: "capture_port") }
    }
    @Published var captureQuic = UserDefaults.standard.object(forKey: "capture_quic") as? Bool ?? false {
        didSet { UserDefaults.standard.set(captureQuic, forKey: "capture_quic") }
    }
    @Published var systemProxyHint = UserDefaults.standard.object(forKey: "system_proxy_hint") as? Bool ?? true {
        didSet { UserDefaults.standard.set(systemProxyHint, forKey: "system_proxy_hint") }
    }
    @Published var urlFilterInput = ""

    private var manager: NETunnelProviderManager?
    private var cursor: String?
    private var requestPageCursors: [String?] = [nil]
    private var urlFilter = ""
    private var requestGeneration: UInt64 = 0
    private var requestPollID: UInt64?
    private var nextRequestPollID: UInt64 = 0
    private var detailGeneration: UInt64 = 0
    private var replayGeneration: UInt64 = 0
    private var manualSendGeneration: UInt64 = 0
    private var manualUploadToken: UInt64?
    private var manualSaving = false
    private var manualSendOffline = false
    private var savedOpenGeneration: UInt64 = 0
    private var savedListGeneration: UInt64 = 0
    private var savedCollectionGeneration: UInt64 = 0
    private var savedListRefreshPending = false
    private var savedCollectionRefreshPending = false
    private var pendingSavedCopy: SavedRequestCopy?
    var manualActivityText: String { manualSaving ? "正在保存请求…" : "正在发送请求…" }

    private func resetSavedRequestUI() {
        savedOpenGeneration &+= 1
        savedListGeneration &+= 1
        savedCollectionGeneration &+= 1
        savedListRefreshPending = false
        savedCollectionRefreshPending = false
        pendingSavedCopy = nil
        savedOpenLoading = false
        savedRequestLoading = false
        savedDeleteBusy = false
        savedRequestError = nil
        savedRequests = []
        savedCollections = []
        savedRequestTotal = 0
        savedRequestQuery = ""
        savedCollectionID = ""
        savedCollectionLoading = false
        savedCollectionSaving = false
        savedRequestOffset = 0
        savedRequestNextOffset = nil
        savedDraft = nil
    }
    private var bodyOffsetHistory: [Int] = []
    private var responseConfig: [String: Any] = [:]
    private var responseVersion = ""
    private var rawDetail = ""
    private var statusObserver: NSObjectProtocol?
    private var stopRequested = false
    private var statusGeneration: UInt64 = 0

    var shouldOfferStop: Bool { connected || tunnelStarting }

    init() { loadProfile() }

    deinit {
        if let statusObserver { NotificationCenter.default.removeObserver(statusObserver) }
    }

    func loadProfile() {
        busy = true
        profileLoaded = false
        message = "正在读取 VPN 配置…"
        NETunnelProviderManager.loadAllFromPreferences { [weak self] managers, error in
            Task { @MainActor [weak self] in
                guard let self else { return }
                if let error {
                    self.busy = false
                    self.message = error.localizedDescription
                    return
                }
                self.manager = managers?.first(where: {
                    ($0.protocolConfiguration as? NETunnelProviderProtocol)?.providerBundleIdentifier == "dev.httpcapture.mobile.ios.PacketTunnel"
                })
                if UserDefaults.standard.object(forKey: "capture_port") == nil,
                   let number = (self.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["port"] as? NSNumber {
                    self.port = number.intValue
                }
                if UserDefaults.standard.object(forKey: "capture_quic") == nil,
                   let number = (self.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["capture_quic"] as? NSNumber {
                    self.captureQuic = number.boolValue
                }
                if UserDefaults.standard.object(forKey: "system_proxy_hint") == nil,
                   let number = (self.manager?.protocolConfiguration as? NETunnelProviderProtocol)?.providerConfiguration?["system_proxy_hint"] as? NSNumber {
                    self.systemProxyHint = number.boolValue
                }
                self.watchStatus()
                self.refreshStatus()
                self.profileLoaded = true
                self.busy = false
            }
        }
    }

    func start() {
        if busy || shouldOfferStop || tunnelStopping { return }
        guard profileLoaded else {
            message = "VPN 配置读取失败，请重试读取后再启动"
            return
        }
        if configurationRequiresRestart {
            message = "配置已导入，请完全关闭并重新打开应用"
            return
        }
        if configurationBusy {
            message = "请等待配置导入或导出完成"
            return
        }
        if sendingManualRequest || savedCollectionSaving || savedDeleteBusy ||
           blockingSaving || responseSaving || trustedCertificatesBusy {
            message = "请等待离线操作完成后再启动 VPN"
            return
        }
        guard (1...65535).contains(port) else {
            message = "端口必须在 1 到 65535 之间"
            return
        }
        busy = true
        let profile = manager ?? NETunnelProviderManager()
        let configuration = NETunnelProviderProtocol()
        configuration.providerBundleIdentifier = "dev.httpcapture.mobile.ios.PacketTunnel"
        configuration.serverAddress = "HTTP Capture"
        configuration.providerConfiguration = [
            "port": port, "capture_quic": captureQuic,
            "system_proxy_hint": systemProxyHint
        ]
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
                        self.stopRequested = false
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

    func beginConfigurationOperation() -> Bool {
        let vpnStatus = manager?.connection.status
        guard !busy, profileLoaded, !configurationBusy, !configurationRequiresRestart,
              !sessionExporting, !sendingManualRequest, !manualSaving,
              !savedCollectionSaving, !savedDeleteBusy,
              !blockingSaving, !responseSaving, !trustedCertificatesBusy,
              !connected, !tunnelStarting, !tunnelStopping,
              vpnStatus == nil || vpnStatus == .disconnected || vpnStatus == .invalid else {
            message = configurationRequiresRestart
                ? "配置已导入，请完全关闭并重新打开应用"
                : "请先停止 VPN，并等待其他请求、规则或导出操作完成"
            return false
        }
        configurationBusy = true
        return true
    }

    func endConfigurationOperation(imported: Bool = false) {
        configurationBusy = false
        if imported {
            configurationRequiresRestart = true
            message = "配置已导入，请完全关闭并重新打开应用"
        }
    }

    func stop() {
        guard shouldOfferStop, !tunnelStopping else { return }
        stopRequested = true
        tunnelStarting = false
        tunnelStopping = true
        manager?.connection.stopVPNTunnel()
        message = "正在停止 VPN…"
    }

    func refreshRequests(loadMore: Bool = false) {
        if loadMore {
            guard !requestPageLoading else { return }
            guard let cursor, Int64(cursor) != nil else { return }
            loadRequestPage(startingAfter: cursor, history: requestPageCursors + [cursor])
        } else {
            loadRequestPage(startingAfter: nil, history: [nil])
        }
    }

    func previousRequestsPage() {
        guard !requestPageLoading, requestPageCursors.count > 1 else { return }
        let history = Array(requestPageCursors.dropLast())
        loadRequestPage(startingAfter: history[history.count - 1], history: history)
    }

    private func loadRequestPage(startingAfter pageCursor: String?, history: [String?]) {
        var command: [String: Any] = ["method": "recent"]
        if !urlFilter.isEmpty { command["urlContains"] = urlFilter }
        if let pageCursor, let value = Int64(pageCursor) {
            command["cursor"] = value
        }
        requestGeneration &+= 1
        let generation = requestGeneration
        requestPollID = nil
        requestPageLoading = true
        send(command, onFailure: { [weak self] in
            guard let self, self.requestGeneration == generation else { return }
            self.requestPageLoading = false
        }) { [weak self] result in
            guard let self, self.requestGeneration == generation else { return }
            self.requestPageLoading = false
            guard let page = result as? [String: Any] else {
                self.message = "请求列表返回格式无效"
                return
            }
            let incoming = ((page["requests"] as? [[String: Any]]) ?? []).compactMap(CapturedRequest.init)
            self.requests = incoming
            self.requestPageCursors = history
            self.cursor = page["nextCursor"] as? String
        }
    }

    func pollRequests() {
        guard connected, requestPageCursors.count == 1,
              !requestPageLoading, requestPollID == nil else { return }
        nextRequestPollID &+= 1
        let pollID = nextRequestPollID
        requestPollID = pollID
        var command: [String: Any] = ["method": "recent"]
        if !urlFilter.isEmpty { command["urlContains"] = urlFilter }
        let generation = requestGeneration
        send(command, onFailure: { [weak self] in
            if self?.requestPollID == pollID { self?.requestPollID = nil }
        }) { [weak self] result in
            guard let self, self.requestPollID == pollID else { return }
            self.requestPollID = nil
            guard self.connected, self.requestGeneration == generation,
                  let page = result as? [String: Any] else { return }
            let incoming = ((page["requests"] as? [[String: Any]]) ?? []).compactMap(CapturedRequest.init)
            self.requests = incoming
            self.cursor = page["nextCursor"] as? String
        }
    }

    var hasMoreRequests: Bool { cursor != nil }
    var hasPreviousRequests: Bool { requestPageCursors.count > 1 }
    var requestPageNumber: Int { requestPageCursors.count }
    var hasActiveUrlFilter: Bool { !urlFilter.isEmpty }

    func applyUrlFilter() {
        let filter = urlFilterInput.trimmingCharacters(in: .whitespacesAndNewlines)
        guard filter.utf8.count <= 2048 else {
            message = "URL 搜索内容不能超过 2048 字节"
            return
        }
        urlFilter = filter
        cursor = nil
        requestPageCursors = [nil]
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

    func cancelRequestCopy() {
        guard replayLoading else { return }
        replayGeneration &+= 1
        replayLoading = false
        replayError = nil
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
                self.message = "抓包内核没有返回阻止规则配置"
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
                          bodyBase64: String? = nil,
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
                  (200...599).contains(code), headers.utf8.count <= 16 * 1024 else {
                message = "状态码需为 200–599，响应头最多 16 KiB"
                return
            }
            let responseBytes: Data
            if let bodyBase64 {
                guard let decoded = try? ResponseBodyCodec.decodeBase64(bodyBase64) else {
                    message = "正文不是有效的 Base64 编码，最多对应 64 KiB 原始字节"
                    return
                }
                responseBytes = decoded
            } else {
                responseBytes = Data(body.utf8)
            }
            guard responseBytes.count <= ResponseBodyCodec.maxBodyBytes else {
                message = "响应正文最多 64 KiB"
                return
            }
            let rows: [[String: Any]]
            do { rows = try RequestHeaderText.parseResponse(headers) }
            catch { message = error.localizedDescription; return }
            rule["action"] = "replace"
            rule["modifications"] = nil
            rule["replacement"] = ["status": Int(code), "headers": rows,
                                   "bodyBase64": responseBytes.base64EncodedString()]
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

    func loadSavedCollections() {
        guard !tunnelStarting, !tunnelStopping else { return }
        if savedCollectionLoading || savedCollectionSaving {
            savedCollectionRefreshPending = true
            return
        }
        savedCollectionRefreshPending = false
        savedCollectionGeneration &+= 1
        let generation = savedCollectionGeneration
        savedCollectionLoading = true
        send(["method": "list_saved_collections"],
             onFailure: { [weak self] in
                 self?.savedRequestError = self?.message
                 self?.finishSavedCollectionLoad()
             }, accept: { [weak self] in self?.savedCollectionGeneration == generation }) { [weak self] value in
            guard let self else { return }
            guard let object = value as? [String: Any],
                  let rows = object["collections"] as? [[String: Any]] else {
                self.savedRequestError = "请求库没有返回集合列表"
                self.finishSavedCollectionLoad()
                return
            }
            self.savedCollections = rows.compactMap(SavedCollectionSummary.init)
            self.finishSavedCollectionLoad()
        }
    }

    private func finishSavedCollectionLoad() {
        savedCollectionLoading = false
        if savedCollectionRefreshPending {
            savedCollectionRefreshPending = false
            loadSavedCollections()
        }
    }

    func changeSavedCollection(action: String, id: String, name: String? = nil) {
        guard !tunnelStarting, !tunnelStopping, !savedCollectionSaving, !savedCollectionLoading,
              !savedRequestLoading, !savedDeleteBusy,
              ["rename", "delete"].contains(action) else { return }
        var request: [String: Any] = ["method": "change_saved_collection", "action": action, "id": id]
        if action == "rename" {
            let value = (name ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !value.isEmpty, value.utf8.count <= 512 else {
                savedRequestError = "集合名称须为 1–512 字节"
                return
            }
            request["name"] = value
        }
        savedCollectionGeneration &+= 1
        let generation = savedCollectionGeneration
        savedCollectionSaving = true
        savedRequestError = nil
        send(request, onFailure: { [weak self] in
            guard let self else { return }
            self.savedCollectionSaving = false
            self.savedRequestError = self.message
            if self.savedCollectionRefreshPending { self.loadSavedCollections() }
        }, accept: { [weak self] in self?.savedCollectionGeneration == generation }) { [weak self] _ in
            guard let self else { return }
            self.savedCollectionSaving = false
            if action == "delete" { self.savedCollectionID = "" }
            self.loadSavedCollections()
            self.loadSavedRequests()
        }
    }

    func loadSavedRequests(offset: Int = 0, query: String? = nil, collectionID: String? = nil) {
        guard !tunnelStarting, !tunnelStopping, !savedDeleteBusy, offset >= 0 else { return }
        if savedRequestLoading {
            if offset == 0 && query == nil && collectionID == nil { savedListRefreshPending = true }
            return
        }
        savedListRefreshPending = false
        let search = (query ?? savedRequestQuery).trimmingCharacters(in: .whitespacesAndNewlines)
        let selectedCollection = collectionID ?? savedCollectionID
        guard search.utf8.count <= 256 else {
            savedRequestError = "搜索词最多 256 字节"
            return
        }
        savedRequestQuery = search
        savedCollectionID = selectedCollection
        savedListGeneration &+= 1
        let generation = savedListGeneration
        savedRequestLoading = true
        savedRequestError = nil
        send(["method": "list_saved_requests", "offset": offset, "query": search,
              "collectionId": selectedCollection],
             onFailure: { [weak self] in
                 guard let self else { return }
                 self.savedRequestError = self.message
                 self.finishSavedListLoad()
             }, accept: { [weak self] in self?.savedListGeneration == generation }) { [weak self] value in
            guard let self else { return }
            guard let page = value as? [String: Any],
                  let rows = page["requests"] as? [[String: Any]] else {
                self.savedRequestError = "请求库没有返回列表"
                self.finishSavedListLoad()
                return
            }
            self.savedRequests = rows.compactMap(SavedRequestSummary.init)
            self.savedRequestTotal = (page["total"] as? NSNumber)?.intValue ?? 0
            self.savedRequestOffset = offset
            self.savedRequestNextOffset = (page["nextOffset"] as? NSNumber)?.intValue
            self.finishSavedListLoad()
        }
    }

    private func finishSavedListLoad() {
        savedRequestLoading = false
        if savedListRefreshPending {
            savedListRefreshPending = false
            loadSavedRequests()
        }
    }

    func deleteSavedRequest(_ id: String) {
        guard !tunnelStarting, !tunnelStopping, !savedDeleteBusy,
              !savedRequestLoading, !savedOpenLoading else { return }
        savedListGeneration &+= 1
        let generation = savedListGeneration
        savedDeleteBusy = true
        savedRequestError = nil
        let nextOffset = savedRequestOffset >= savedRequestTotal - 1
            ? max(0, savedRequestOffset - 100) : savedRequestOffset
        send(["method": "delete_saved_request", "id": id],
             onFailure: { [weak self] in
                 guard let self else { return }
                 self.savedDeleteBusy = false
                 self.savedRequestError = self.message
             }, accept: { [weak self] in self?.savedListGeneration == generation }) { [weak self] _ in
            guard let self else { return }
            self.savedDeleteBusy = false
            self.message = "已删除保存的请求"
            self.loadSavedCollections()
            self.loadSavedRequests(offset: nextOffset)
        }
    }

    func openSavedRequest(_ id: String) {
        guard !tunnelStarting, !tunnelStopping, !savedOpenLoading else { return }
        savedOpenGeneration &+= 1
        let generation = savedOpenGeneration
        pendingSavedCopy = nil
        savedOpenLoading = true
        savedRequestError = nil
        send(["method": "open_saved_request", "id": id],
             onFailure: { [weak self] in
                 guard let self else { return }
                 self.savedOpenLoading = false
                 self.savedRequestError = self.message
             },
             accept: { [weak self] in self?.savedOpenGeneration == generation }) { [weak self] value in
            guard let self else { return }
            do {
                guard let object = value as? [String: Any] else {
                    throw SavedRequestCopy.CopyError.invalid("请求库没有返回完整请求")
                }
                let copy = try SavedRequestCopy(object)
                if copy.totalBytes == 0 {
                    self.savedDraft = try copy.draft()
                    self.savedOpenLoading = false
                } else {
                    self.pendingSavedCopy = copy
                    self.readSavedBody(generation: generation)
                }
            } catch {
                self.savedOpenLoading = false
                self.savedRequestError = error.localizedDescription
            }
        }
    }

    private func readSavedBody(generation: UInt64) {
        guard savedOpenGeneration == generation, let copy = pendingSavedCopy else { return }
        send(["method": "saved_request_body", "readToken": copy.readToken, "offset": copy.offset],
             onFailure: { [weak self] in
                 guard let self else { return }
                 self.savedOpenLoading = false
                 self.pendingSavedCopy = nil
                 self.savedRequestError = self.message
             },
             accept: { [weak self] in self?.savedOpenGeneration == generation }) { [weak self] value in
            guard let self else { return }
            do {
                guard let part = value as? [String: Any], var pending = self.pendingSavedCopy else {
                    throw SavedRequestCopy.CopyError.invalid("保存的正文读取会话已结束")
                }
                let complete = try pending.append(part)
                self.pendingSavedCopy = pending
                if complete {
                    self.savedDraft = try pending.draft()
                    self.pendingSavedCopy = nil
                    self.savedOpenLoading = false
                } else {
                    self.readSavedBody(generation: generation)
                }
            } catch {
                self.pendingSavedCopy = nil
                self.savedOpenLoading = false
                self.savedRequestError = error.localizedDescription
            }
        }
    }

    func sendManualRequest(method: String, url: String, engine: ManualSendEngine,
                           headers: String, body: PreparedRequestBody) {
        guard !tunnelStarting, !tunnelStopping, !sendingManualRequest else { return }
        manualSaving = false
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
        var headerRows: [[String: Any]]
        do {
            headerRows = try RequestHeaderText.parse(headers)
        } catch {
            message = error.localizedDescription
            return
        }
        headerRows = RequestHeaderText.applyingContentType(body.contentType,
                                                            replace: body.replaceContentType,
                                                            to: headerRows)
        var draft: [String: Any] = ["method": method, "url": url, "engine": engine.rawValue,
                                    "headers": headerRows, "bodyBase64": ""]
        manualSendGeneration &+= 1
        let generation = manualSendGeneration
        sendingManualRequest = true
        manualSendOffline = !connected
        canCancelManualUpload = false
        manualUploadToken = nil
        message = "正在发送请求…"
        if !connected || body.bytes.count <= 48 * 1024 {
            draft["bodyBase64"] = body.bytes.base64EncodedString()
            let direct: [String: Any] = ["method": "send_request", "request": draft]
            if let data = try? JSONSerialization.data(withJSONObject: direct),
               data.count <= (connected ? 128 * 1024 : 2 * 1024 * 1024) {
                send(direct, onFailure: { [weak self] in
                    self?.sendingManualRequest = false
                    self?.manualSendOffline = false
                },
                     accept: { [weak self] in self?.manualSendGeneration == generation }) {
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
        send(begin, onFailure: { [weak self] in self?.sendingManualRequest = false },
             accept: { [weak self] in self?.manualSendGeneration == generation }) { [weak self] value in
            guard let self else { return }
            let reply = value as? [String: Any]
            guard let token = (reply?["token"] as? NSNumber)?.uint64Value,
                  (reply?["nextOffset"] as? NSNumber)?.intValue == 0 else {
                if let token = (reply?["token"] as? NSNumber)?.uint64Value {
                    self.cancelUploadToken(token)
                }
                self.sendingManualRequest = false
                self.message = "VPN 扩展没有创建请求上传会话"
                return
            }
            self.manualUploadToken = token
            self.canCancelManualUpload = true
            self.appendManualBody(body.bytes, token: token, offset: 0, generation: generation)
        }
    }

    func saveManualRequest(id: String?, name: String, method: String, url: String,
                           engine: ManualSendEngine, headers: String, body: PreparedRequestBody,
                           collectionID: String? = nil, newCollectionName: String? = nil) {
        guard !tunnelStarting, !tunnelStopping, !sendingManualRequest else { return }
        let selectedCollection = collectionID?.isEmpty == false ? collectionID : nil
        let createdCollection = newCollectionName?.trimmingCharacters(in: .whitespacesAndNewlines)
        guard selectedCollection == nil || createdCollection == nil else {
            message = "请选择已有集合或新建集合"
            return
        }
        if let createdCollection, createdCollection.isEmpty || createdCollection.utf8.count > 512 {
            message = "集合名称须为 1–512 字节"
            return
        }
        let name = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty, name.utf8.count <= 512 else {
            message = "请求名称须为 1–512 字节"
            return
        }
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
        var headerRows: [[String: Any]]
        do { headerRows = try RequestHeaderText.parse(headers) }
        catch { message = error.localizedDescription; return }
        headerRows = RequestHeaderText.applyingContentType(body.contentType,
                                                            replace: body.replaceContentType,
                                                            to: headerRows)
        var draft: [String: Any] = ["method": method, "url": url, "engine": engine.rawValue,
                                    "headers": headerRows, "bodyBase64": ""]
        manualSendGeneration &+= 1
        let generation = manualSendGeneration
        manualSaving = true
        sendingManualRequest = true
        manualSendOffline = !connected
        canCancelManualUpload = false
        manualUploadToken = nil
        message = "正在保存请求…"
        if !connected || body.bytes.count <= 48 * 1024 {
            draft["bodyBase64"] = body.bytes.base64EncodedString()
            var direct: [String: Any] = ["method": "save_request", "name": name, "request": draft]
            if let id { direct["id"] = id }
            if let selectedCollection { direct["collectionId"] = selectedCollection }
            if let createdCollection { direct["newCollectionName"] = createdCollection }
            if let data = try? JSONSerialization.data(withJSONObject: direct),
               data.count <= (connected ? 128 * 1024 : 2 * 1024 * 1024) {
                send(direct, onFailure: { [weak self] in self?.failManualSave() },
                     accept: { [weak self] in self?.manualSendGeneration == generation }) {
                    [weak self] value in self?.finishManualRequest(value)
                }
                return
            }
            draft["bodyBase64"] = ""
        }
        var begin: [String: Any] = ["method": "begin_saved_request_upload", "name": name,
                                    "request": draft, "totalBytes": body.bytes.count]
        if let id { begin["id"] = id }
        if let selectedCollection { begin["collectionId"] = selectedCollection }
        if let createdCollection { begin["newCollectionName"] = createdCollection }
        guard let data = try? JSONSerialization.data(withJSONObject: begin), data.count <= 128 * 1024 else {
            failManualSave()
            message = "请求头超过 VPN 扩展消息上限 128 KiB"
            return
        }
        send(begin, onFailure: { [weak self] in self?.failManualSave() },
             accept: { [weak self] in self?.manualSendGeneration == generation }) { [weak self] value in
            guard let self else { return }
            let reply = value as? [String: Any]
            guard let token = (reply?["token"] as? NSNumber)?.uint64Value,
                  (reply?["nextOffset"] as? NSNumber)?.intValue == 0 else {
                if let token = (reply?["token"] as? NSNumber)?.uint64Value {
                    self.cancelUploadToken(token)
                }
                self.failManualSave()
                self.message = "VPN 扩展没有创建保存上传会话"
                return
            }
            self.manualUploadToken = token
            self.canCancelManualUpload = true
            self.appendManualBody(body.bytes, token: token, offset: 0, generation: generation)
        }
    }

    private func failManualSave() {
        sendingManualRequest = false
        manualSendOffline = false
        canCancelManualUpload = false
        manualSaving = false
        manualUploadToken = nil
    }

    func cancelManualUpload() {
        guard sendingManualRequest, canCancelManualUpload,
              let token = manualUploadToken else { return }
        manualSendGeneration &+= 1
        sendingManualRequest = false
        canCancelManualUpload = false
        manualUploadToken = nil
        manualSaving = false
        cancelUploadToken(token)
        message = "已取消正文上传；请求尚未提交"
    }

    private func cancelUploadToken(_ token: UInt64) {
        guard let session = manager?.connection as? NETunnelProviderSession,
              let data = try? JSONSerialization.data(withJSONObject:
                ["method": "cancel_request_upload", "token": token]) else { return }
        try? session.sendProviderMessage(data) { _ in }
    }

    private func failManualUpload(_ token: UInt64) {
        sendingManualRequest = false
        canCancelManualUpload = false
        manualUploadToken = nil
        manualSaving = false
        cancelUploadToken(token)
    }

    private func abandonManualSend() {
        guard sendingManualRequest else { return }
        let token = manualUploadToken
        manualSendGeneration &+= 1
        sendingManualRequest = false
        manualSendOffline = false
        canCancelManualUpload = false
        manualUploadToken = nil
        manualSaving = false
        if let token { cancelUploadToken(token) }
    }

    private func appendManualBody(_ body: Data, token: UInt64, offset: Int,
                                  generation: UInt64) {
        guard sendingManualRequest, manualSendGeneration == generation else { return }
        if offset == body.count {
            canCancelManualUpload = false
            send(["method": manualSaving ? "commit_saved_request_upload" : "commit_request_upload",
                  "token": token],
                 onFailure: { [weak self] in self?.failManualUpload(token) },
                 accept: { [weak self] in self?.manualSendGeneration == generation }) {
                [weak self] value in self?.finishManualRequest(value)
            }
            return
        }
        let end = min(offset + 16 * 1024, body.count)
        let command: [String: Any] = ["method": "append_request_upload", "token": token,
                                      "offset": offset,
                                      "bodyBase64": body.subdata(in: offset..<end).base64EncodedString()]
        send(command, onFailure: { [weak self] in self?.failManualUpload(token) },
             accept: { [weak self] in self?.manualSendGeneration == generation }) { [weak self] value in
            guard let self else { return }
            guard let result = value as? [String: Any],
                  (result["nextOffset"] as? NSNumber)?.intValue == end else {
                self.failManualUpload(token)
                self.message = "请求正文上传位置不一致，请重试"
                return
            }
            self.appendManualBody(body, token: token, offset: end, generation: generation)
        }
    }

    private func finishManualRequest(_ value: Any?) {
        sendingManualRequest = false
        manualSendOffline = false
        canCancelManualUpload = false
        manualUploadToken = nil
        if manualSaving {
            manualSaving = false
            guard let result = value as? [String: Any], result["id"] as? String != nil else {
                message = "请求库没有返回保存结果"
                return
            }
            message = "请求已保存"
            manualSaveCounter &+= 1
            loadSavedCollections()
            loadSavedRequests()
            return
        }
        guard let result = value as? [String: Any] else {
            message = "VPN 扩展没有返回发送结果"
            return
        }
        if let error = result["error"] as? String { message = "请求失败：" + error }
        else { message = "请求已发送，状态 \(result["status"] ?? "未知")" }
        refreshRequests()
        if let id = result["id"] as? String {
            manualResultID = id
            showDetail(id: id)
        }
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

    func exportSession() {
        guard !sessionExporting, !tunnelStarting, !tunnelStopping else { return }
        if let old = sessionExportURL { try? FileManager.default.removeItem(at: old) }
        sessionExportURL = nil
        sessionExporting = true
        sessionExportProgress = 0
        sessionExportGeneration &+= 1
        let generation = sessionExportGeneration
        message = "正在导出抓包记录…"
        if !connected {
            exportOfflineSession(generation: generation)
            return
        }
        send(["method": "begin_session_export"], onFailure: { [weak self] in
            guard let self else { return }
            if self.sessionExportCancelPending && self.sessionExportGeneration == (generation &+ 1) {
                self.sessionExportCancelPending = false
                self.sessionExporting = false
                self.message = "已取消导出抓包记录"
                return
            }
            guard self.sessionExportGeneration == generation else { return }
            self.sessionExporting = false
        }) { [weak self] value in
            guard let self else { return }
            guard let info = value as? [String: Any],
                  let token = (info["token"] as? NSNumber)?.uint64Value,
                  let total = (info["totalBytes"] as? NSNumber)?.uint64Value,
                  total > 0,
                  let count = info["count"] as? Int else {
                if self.sessionExportGeneration == generation {
                    self.sessionExporting = false
                    self.message = "VPN 扩展没有返回导出信息"
                } else if self.sessionExportCancelPending && self.sessionExportGeneration == (generation &+ 1) {
                    self.sessionExportCancelPending = false
                    self.sessionExporting = false
                    self.message = "已取消导出抓包记录"
                }
                return
            }
            guard self.sessionExportGeneration == generation else {
                // A response from a previous VPN session must not send a finish
                // command to the current session, where token values may repeat.
                guard self.sessionExportCancelPending,
                      self.sessionExportGeneration == (generation &+ 1) else { return }
                let cancelledGeneration = self.sessionExportGeneration
                self.finishSessionExport(token: token) { [weak self] in
                    guard let self, self.sessionExportGeneration == cancelledGeneration,
                          self.sessionExportCancelPending else { return }
                    self.sessionExportCancelPending = false
                    self.sessionExporting = false
                    self.message = "已取消导出抓包记录"
                }
                return
            }
            let url = FileManager.default.temporaryDirectory
                .appendingPathComponent("http-capture-\(UUID().uuidString).json")
            guard FileManager.default.createFile(atPath: url.path, contents: nil),
                  let file = try? FileHandle(forWritingTo: url) else {
                self.message = "无法创建临时导出文件"
                try? FileManager.default.removeItem(at: url)
                self.sessionExportCancelPending = true
                let finishingGeneration = self.sessionExportGeneration
                self.finishSessionExport(token: token) { [weak self] in
                    guard self?.sessionExportGeneration == finishingGeneration else { return }
                    self?.sessionExportCancelPending = false
                    self?.sessionExporting = false
                }
                return
            }
            self.sessionExportToken = token
            self.sessionExportFile = file
            self.sessionExportTempURL = url
            self.readSessionExport(token: token, total: total, offset: 0,
                                   count: count, file: file, url: url, generation: generation)
        }
    }

    private func exportOfflineSession(generation: UInt64) {
        sessionExportOffline = true
        guard let shared = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.dev.httpcapture.mobile.ios") else {
            sessionExportOffline = false
            sessionExporting = false
            message = "App Group 未启用，无法导出离线记录"
            return
        }
        let directory = shared.appendingPathComponent("HTTP Capture", isDirectory: true).path
        guard FileManager.default.fileExists(atPath: directory) else {
            sessionExportOffline = false
            sessionExporting = false
            message = "请先启动一次 VPN，完成共享数据目录初始化或旧记录迁移"
            return
        }
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("http-capture-\(UUID().uuidString).json")
        DispatchQueue.global(qos: .userInitiated).async {
            let response: Data? = directory.withCString { path in
                url.path.withCString { output in
                    guard let pointer = hc_ios_offline_export_session(path, output) else { return nil as Data? }
                    defer { hc_ios_free_string(pointer) }
                    return Data(String(cString: pointer).utf8)
                }
            }
            Task { @MainActor [weak self] in
                guard let self else { try? FileManager.default.removeItem(at: url); return }
                guard self.sessionExportGeneration == generation else {
                    try? FileManager.default.removeItem(at: url)
                    if self.sessionExportOffline && self.sessionExportCancelPending {
                        self.sessionExportOffline = false
                        self.sessionExportCancelPending = false
                        self.sessionExporting = false
                        self.message = "已取消导出抓包记录"
                    }
                    return
                }
                self.sessionExportOffline = false
                self.sessionExporting = false
                guard let response,
                      let result = (try? JSONSerialization.jsonObject(with: response)) as? [String: Any],
                      result["ok"] as? Bool == true,
                      let value = result["value"] as? [String: Any],
                      let count = value["count"] as? Int else {
                    try? FileManager.default.removeItem(at: url)
                    let error = response.flatMap {
                        (try? JSONSerialization.jsonObject(with: $0)) as? [String: Any]
                    }
                    self.message = error?["error"] as? String ?? "离线导出失败"
                    return
                }
                self.sessionExportProgress = 100
                self.sessionExportURL = url
                self.message = "已导出 \(count) 条抓包记录"
            }
        }
    }

    func cancelSessionExport() {
        guard sessionExporting, !sessionExportCancelPending else { return }
        if sessionExportOffline {
            sessionExportGeneration &+= 1
            sessionExportCancelPending = true
            message = "正在取消导出抓包记录…"
            return
        }
        if let token = sessionExportToken,
           let file = sessionExportFile,
           let url = sessionExportTempURL {
            endSessionExport(token: token, file: file, url: url, success: false, cancelled: true)
        } else {
            sessionExportGeneration &+= 1
            sessionExportCancelPending = true
        }
        message = "正在取消导出抓包记录…"
    }

    private func readSessionExport(token: UInt64, total: UInt64, offset: UInt64,
                                   count: Int, file: FileHandle, url: URL, generation: UInt64) {
        send(["method": "read_session_export", "token": token, "offset": offset],
             onFailure: { [weak self] in
                 guard let self, self.sessionExportGeneration == generation else { return }
                 self.endSessionExport(token: token, file: file, url: url, success: false)
             }, accept: { [weak self] in self?.sessionExportGeneration == generation }) { [weak self] value in
            guard let self, self.sessionExportGeneration == generation else { return }
            guard let chunk = value as? [String: Any],
                  let returnedOffset = (chunk["offset"] as? NSNumber)?.uint64Value,
                  returnedOffset == offset,
                  let returnedTotal = (chunk["totalBytes"] as? NSNumber)?.uint64Value,
                  returnedTotal == total,
                  let encoded = chunk["chunkBase64"] as? String,
                  let bytes = Data(base64Encoded: encoded),
                  bytes.count <= 32 * 1024,
                  UInt64(bytes.count) <= total - offset else {
                self.message = "导出数据不完整"
                self.endSessionExport(token: token, file: file, url: url, success: false)
                return
            }
            do { try file.write(contentsOf: bytes) }
            catch {
                self.message = "写入导出文件失败：\(error.localizedDescription)"
                self.endSessionExport(token: token, file: file, url: url, success: false)
                return
            }
            let next = offset + UInt64(bytes.count)
            self.sessionExportProgress = Int(Double(next) / Double(total) * 100)
            if next == total {
                self.endSessionExport(token: token, file: file, url: url, success: true)
                self.message = "已导出 \(count) 条抓包记录"
            } else if next > offset {
                self.readSessionExport(token: token, total: total, offset: next,
                                       count: count, file: file, url: url, generation: generation)
            } else {
                self.message = "导出数据未前进"
                self.endSessionExport(token: token, file: file, url: url, success: false)
            }
        }
    }

    private func endSessionExport(token: UInt64, file: FileHandle, url: URL,
                                  success: Bool, cancelled: Bool = false) {
        sessionExportGeneration &+= 1
        try? file.close()
        if success { sessionExportURL = url }
        else { try? FileManager.default.removeItem(at: url) }
        sessionExportToken = nil
        sessionExportFile = nil
        sessionExportTempURL = nil
        sessionExportCancelPending = true
        let finishingGeneration = sessionExportGeneration
        finishSessionExport(token: token) { [weak self] in
            guard self?.sessionExportGeneration == finishingGeneration else { return }
            self?.sessionExportCancelPending = false
            self?.sessionExporting = false
            if cancelled { self?.message = "已取消导出抓包记录" }
        }
    }

    private func finishSessionExport(token: UInt64, completion: @escaping () -> Void) {
        guard let session = manager?.connection as? NETunnelProviderSession,
              let data = try? JSONSerialization.data(withJSONObject:
                  ["method": "finish_session_export", "token": token]) else {
            completion()
            return
        }
        do {
            try session.sendProviderMessage(data) { _ in
                Task { @MainActor in completion() }
            }
        } catch { completion() }
    }

    private func abandonSessionExport() {
        guard sessionExporting else { return }
        sessionExportGeneration &+= 1
        try? sessionExportFile?.close()
        if let url = sessionExportTempURL { try? FileManager.default.removeItem(at: url) }
        if let token = sessionExportToken { finishSessionExport(token: token) { } }
        sessionExportFile = nil
        sessionExportTempURL = nil
        sessionExportToken = nil
        sessionExportOffline = false
        sessionExportCancelPending = false
        sessionExporting = false
        sessionExportProgress = 0
    }

    func refreshTrustedCertificates() {
        send(["method": "certificates"]) { [weak self] value in
            guard let self, let snapshot = value as? [String: Any] else { return }
            self.trustedCertificatesVersion = snapshot["version"] as? String ?? ""
            self.trustedCertificates = (snapshot["entries"] as? [[String: Any]] ?? [])
                .compactMap(TrustedCertificate.init)
        }
    }

    func reportTrustedCertificateError(_ error: String) { message = error }

    func importTrustedCertificate(data: Data, name: String, host: String, port: Int) {
        guard !trustedCertificatesBusy else { return }
        guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              (1...65535).contains(port), !trustedCertificatesVersion.isEmpty else {
            message = "请填写证书名称和有效端口，并刷新证书列表"
            return
        }
        guard data.count <= 64 * 1024 else {
            message = "iOS 证书文件暂限 64 KiB；请使用单张 CA 的 PEM 或 DER 文件"
            return
        }
        trustedCertificatesBusy = true
        let input: [String: Any] = ["name": name, "kind": "trust", "host": host,
                                    "port": port, "certificateBase64": data.base64EncodedString(),
                                    "expectedVersion": trustedCertificatesVersion]
        send(["method": "import_certificate", "input": input], onFailure: { [weak self] in
            self?.trustedCertificatesBusy = false
            self?.refreshTrustedCertificates()
        }) { [weak self] _ in
            self?.trustedCertificatesBusy = false
            self?.message = "上游 CA 已导入，请在列表中启用"
            self?.refreshTrustedCertificates()
        }
    }

    func changeTrustedCertificate(_ certificate: TrustedCertificate, delete: Bool = false) {
        guard !trustedCertificatesBusy, !trustedCertificatesVersion.isEmpty else { return }
        trustedCertificatesBusy = true
        let input: [String: Any] = ["action": delete ? "delete" : "enable",
                                    "id": certificate.id, "enabled": !certificate.enabled,
                                    "expectedVersion": trustedCertificatesVersion]
        send(["method": "change_certificate", "input": input], onFailure: { [weak self] in
            self?.trustedCertificatesBusy = false
            self?.refreshTrustedCertificates()
        }) { [weak self] _ in
            self?.trustedCertificatesBusy = false
            self?.message = "上游 CA 已更新；新连接将使用此设置"
            self?.refreshTrustedCertificates()
        }
    }

    private func send(_ command: [String: Any], onFailure: (() -> Void)? = nil,
                      accept: (() -> Bool)? = nil,
                      completion: @escaping (Any?) -> Void) {
        guard let data = try? JSONSerialization.data(withJSONObject: command) else {
            message = "请求命令无法编码"
            onFailure?()
            return
        }
        let deliver: (Data?) -> Void = { [weak self] response in
            Task { @MainActor [weak self] in
                guard let self else { return }
                guard accept?() ?? true else { return }
                guard let response,
                      let object = (try? JSONSerialization.jsonObject(with: response)) as? [String: Any] else {
                    self.message = "抓包内核没有返回结果"
                    onFailure?()
                    return
                }
                if object["ok"] as? Bool == true { completion(object["value"]) }
                else {
                    self.message = object["error"] as? String ?? "请求失败"
                    onFailure?()
                }
            }
        }
        if !connected {
            guard let method = command["method"] as? String,
                  ["recent", "metadata", "body_preview", "list_saved_collections",
                   "change_saved_collection", "list_saved_requests", "save_request",
                   "delete_saved_request", "open_saved_request", "saved_request_body",
                   "send_request", "blocking", "configure_blocking",
                   "response_rewrite", "configure_response_rewrite", "certificate",
                   "certificates", "import_certificate", "change_certificate"].contains(method),
                  !tunnelStarting, !tunnelStopping else {
                message = "请先启动 VPN"
                onFailure?()
                return
            }
            guard let shared = FileManager.default.containerURL(
                forSecurityApplicationGroupIdentifier: "group.dev.httpcapture.mobile.ios") else {
                message = "App Group 未启用，无法读取离线抓包记录"
                onFailure?()
                return
            }
            let directory = shared.appendingPathComponent("HTTP Capture", isDirectory: true).path
            guard FileManager.default.fileExists(atPath: directory) else {
                message = "请先启动一次 VPN，完成共享数据目录初始化或旧记录迁移"
                onFailure?()
                return
            }
            DispatchQueue.global(qos: .userInitiated).async {
                let response = directory.withCString { path in
                    String(decoding: data, as: UTF8.self).withCString { request in
                        guard let pointer = hc_ios_offline_command(path, request) else { return nil as Data? }
                        defer { hc_ios_free_string(pointer) }
                        return Data(String(cString: pointer).utf8)
                    }
                }
                deliver(response)
            }
            return
        }
        guard let session = manager?.connection as? NETunnelProviderSession else {
            message = "VPN 会话尚未就绪"
            onFailure?()
            return
        }
        do {
            try session.sendProviderMessage(data) { response in deliver(response) }
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
        statusGeneration &+= 1
        switch manager?.connection.status {
        case .connected:
            if sessionExportOffline { abandonSessionExport() }
            if manualSendOffline { abandonManualSend() }
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
            abandonSessionExport()
            abandonManualSend()
            resetSavedRequestUI()
            if tunnelStopping {
                connected = false
                message = "正在停止 VPN…"
                return
            }
            connected = false
            tunnelStarting = true
            tunnelStopping = false
            requestGeneration &+= 1
            requestPollID = nil
            cursor = nil
            requestPageCursors = [nil]
            requestPageLoading = false
            message = "正在建立 VPN…"
        case .disconnecting:
            abandonSessionExport()
            abandonManualSend()
            resetSavedRequestUI()
            connected = false
            tunnelStarting = false
            tunnelStopping = true
            requestGeneration &+= 1
            requestPollID = nil
            cursor = nil
            requestPageCursors = [nil]
            requestPageLoading = false
            message = "正在停止 VPN…"
        default:
            let wasActive = connected || tunnelStarting || tunnelStopping
            let unexpectedDisconnect = (wasActive && !stopRequested) ||
                (!wasActive && message == "VPN 意外断开")
            let previousDisconnectMessage = !wasActive && message.hasPrefix("VPN 意外断开：") ? message : nil
            stopRequested = false
            if !sessionExportOffline { abandonSessionExport() }
            if !manualSendOffline { abandonManualSend() }
            if wasActive { resetSavedRequestUI() }
            connected = false
            tunnelStarting = false
            tunnelStopping = false
            requestGeneration &+= 1
            requestPollID = nil
            cursor = nil
            requestPageCursors = [nil]
            requestPageLoading = false
            if !manualSendOffline && !sessionExportOffline {
                message = unexpectedDisconnect ? "VPN 意外断开" : (previousDisconnectMessage ?? "VPN 未启动")
                if unexpectedDisconnect, let connection = manager?.connection {
                    let generation = statusGeneration
                    connection.fetchLastDisconnectError { [weak self] error in
                        Task { @MainActor [weak self] in
                            guard let self,
                                  self.statusGeneration == generation,
                                  (self.manager?.connection.status == .disconnected ||
                                   self.manager?.connection.status == .invalid),
                                  self.message == "VPN 意外断开" else { return }
                            if let error { self.message = "VPN 意外断开：\(error.localizedDescription)" }
                        }
                    }
                }
            }
            clearDetail()
            refreshRequests()
        }
    }
}
