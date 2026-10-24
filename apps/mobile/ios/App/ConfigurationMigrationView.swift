import Foundation
import SwiftUI
import UniformTypeIdentifiers

private struct ConfigurationSection: Identifiable {
    let id: String
    let count: Int
}

struct ConfigurationMigrationView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var includeRequestData = false
    @State private var includeCode = false
    @State private var includeWorkspace = false
    @State private var includeProxyUsernames = false
    @State private var exportPassword = ""
    @State private var importPassword = ""
    @State private var exportURL: URL?
    @State private var importURL: URL?
    @State private var preview: [String: Any]?
    @State private var selectedSections = Set<String>()
    @State private var showingImporter = false
    @State private var confirmingImport = false
    @State private var message = ""

    private var sections: [ConfigurationSection] {
        (preview?["sections"] as? [[String: Any]] ?? []).compactMap { section in
            guard let id = section["id"] as? String else { return nil }
            return ConfigurationSection(id: id, count: section["count"] as? Int ?? 0)
        }
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("导出") {
                    Toggle("包含请求头值、正文、变量和响应修改规则", isOn: $includeRequestData)
                    Toggle("包含脚本、插件和代码模板", isOn: $includeCode)
                    Toggle("包含桌面请求工作区", isOn: $includeWorkspace)
                    Toggle("包含上游代理用户名", isOn: $includeProxyUsernames)
                    SecureField("加密密码（留空导出普通 JSON）", text: $exportPassword)
                        .textContentType(.newPassword)
                    Button("生成配置文件") { exportConfiguration() }
                        .disabled(tunnel.configurationBusy || tunnel.configurationRequiresRestart)
                    if let exportURL {
                        ShareLink("分享或保存配置文件", item: exportURL)
                    }
                    Text("默认排除敏感请求数据和代码；代理密码、CA 私钥、证书信任与流量记录不会导出。加密密码需为 12–1024 字节。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("导入") {
                    SecureField("文件密码（未加密则留空）", text: $importPassword)
                    Button("选择配置文件并预览") { showingImporter = true }
                        .disabled(tunnel.configurationBusy || tunnel.configurationRequiresRestart)
                    if let preview {
                        Text("来源版本：\(preview["sourceVersion"] as? String ?? "未知")")
                            .font(.footnote).foregroundStyle(.secondary)
                        ForEach(sections) { section in
                            Toggle("\(sectionLabel(section.id)) · \(section.count) 项",
                                   isOn: Binding(
                                    get: { selectedSections.contains(section.id) },
                                    set: { selected in
                                        if selected { selectedSections.insert(section.id) }
                                        else { selectedSections.remove(section.id) }
                                    }))
                        }
                        if let warnings = preview["warnings"] as? [String] {
                            ForEach(Array(warnings.prefix(6)), id: \.self) { warning in
                                Text(warning).font(.footnote).foregroundStyle(.secondary)
                            }
                        }
                        Button("导入选中的类别", role: .destructive) {
                            confirmingImport = true
                        }.disabled(selectedSections.isEmpty || tunnel.configurationBusy ||
                                   tunnel.configurationRequiresRestart)
                    }
                    Text("导入会替换所选类别，保存本机恢复点，并关闭抓包及规则开关。提交后需完全关闭应用再重新打开。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                if tunnel.configurationBusy { ProgressView("正在处理配置…") }
                if !message.isEmpty {
                    Section("结果") { Text(message).textSelection(.enabled) }
                }
            }
            .navigationTitle("全局配置")
            .toolbar { Button("关闭") { dismiss() }.disabled(tunnel.configurationBusy) }
            .interactiveDismissDisabled(tunnel.configurationBusy)
            .fileImporter(isPresented: $showingImporter, allowedContentTypes: [.item]) { result in
                switch result {
                case .success(let url): prepareImportFile(url)
                case .failure(let error): message = error.localizedDescription
                }
            }
            .alert("确认替换配置？", isPresented: $confirmingImport) {
                Button("取消", role: .cancel) { }
                Button("导入", role: .destructive) { importConfiguration() }
            } message: {
                Text("所选类别将被替换，原配置会保存为恢复点。导入完成后请完全关闭并重新打开应用。")
            }
            .onDisappear {
                if let importURL { try? FileManager.default.removeItem(at: importURL) }
            }
        }
    }

    private func dataDirectory() throws -> URL {
        guard let shared = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.dev.httpcapture.mobile.ios") else {
            throw NSError(domain: "HTTP Capture", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "App Group 未启用，无法访问共享配置"])
        }
        let directory = shared.appendingPathComponent("HTTP Capture", isDirectory: true)
        guard FileManager.default.fileExists(atPath: directory.path) else {
            throw NSError(domain: "HTTP Capture", code: 2,
                          userInfo: [NSLocalizedDescriptionKey:
                            "请先启动一次 VPN，完成共享数据目录初始化或旧记录迁移，再停止 VPN 导入配置"])
        }
        return directory
    }

    private static func nativeResult(_ pointer: UnsafeMutablePointer<CChar>?) -> [String: Any] {
        guard let pointer else { return ["ok": false, "error": "原生内核未返回结果"] }
        defer { hc_ios_free_string(pointer) }
        let data = Data(String(cString: pointer).utf8)
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            ?? ["ok": false, "error": "原生内核返回无效 JSON"]
    }

    private func exportConfiguration() {
        let password = exportPassword
        let passwordBytes = password.utf8.count
        guard passwordBytes == 0 || (12...1024).contains(passwordBytes) else {
            message = "加密密码需为 12–1024 字节；或留空导出普通 JSON"
            return
        }
        guard tunnel.beginConfigurationOperation() else { return }
        do {
            let directory = try dataDirectory()
            let options: [String: Bool] = [
                "includeRequestData": includeRequestData, "includeCode": includeCode,
                "includeWorkspace": includeWorkspace, "includeProxyUsernames": includeProxyUsernames
            ]
            let optionsData = try JSONSerialization.data(withJSONObject: options)
            guard let optionsJSON = String(data: optionsData, encoding: .utf8) else {
                throw NSError(domain: "HTTP Capture", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: "无法编码导出选项"])
            }
            let url = FileManager.default.temporaryDirectory.appendingPathComponent(
                "http-capture-config-\(UUID().uuidString)" + (password.isEmpty ? ".json" : ".hccfg"))
            let previousURL = exportURL
            exportURL = nil
            if let previousURL { try? FileManager.default.removeItem(at: previousURL) }
            message = "正在导出配置…"
            DispatchQueue.global(qos: .userInitiated).async {
                let response = directory.path.withCString { path in
                    url.path.withCString { output in
                        optionsJSON.withCString { options in
                            password.withCString { secret in
                                Self.nativeResult(hc_ios_config_export(path, output, options, secret))
                            }
                        }
                    }
                }
                Task { @MainActor in
                    tunnel.endConfigurationOperation()
                    if response["ok"] as? Bool == true {
                        exportURL = url
                        message = "配置已生成。分享前请检查规则、名称和 URL；密码请另行保管。"
                    } else {
                        try? FileManager.default.removeItem(at: url)
                        message = response["error"] as? String ?? "配置导出失败"
                    }
                }
            }
        } catch {
            tunnel.endConfigurationOperation()
            message = error.localizedDescription
        }
    }

    private func prepareImportFile(_ source: URL) {
        guard !tunnel.configurationBusy, !tunnel.configurationRequiresRestart else { return }
        let accessed = source.startAccessingSecurityScopedResource()
        defer { if accessed { source.stopAccessingSecurityScopedResource() } }
        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: source.path)
            let length = (attributes[.size] as? NSNumber)?.int64Value ?? 0
            guard length > 0 && length <= 24 * 1024 * 1024 else {
                message = "配置文件需为 1 字节到 24 MiB"
                return
            }
            let temporary = FileManager.default.temporaryDirectory
                .appendingPathComponent("http-capture-import-\(UUID().uuidString).hccfg")
            try FileManager.default.copyItem(at: source, to: temporary)
            if let importURL { try? FileManager.default.removeItem(at: importURL) }
            importURL = temporary
            preview = nil
            selectedSections = []
            loadPreview(temporary)
        } catch {
            message = "读取配置文件失败：\(error.localizedDescription)"
        }
    }

    private func loadPreview(_ url: URL) {
        guard tunnel.beginConfigurationOperation() else { return }
        let password = importPassword
        do {
            let directory = try dataDirectory()
            message = "正在预览配置…"
            DispatchQueue.global(qos: .userInitiated).async {
                let response = directory.path.withCString { path in
                    url.path.withCString { input in
                        password.withCString { secret in
                            Self.nativeResult(hc_ios_config_preview(path, input, secret))
                        }
                    }
                }
                Task { @MainActor in
                    tunnel.endConfigurationOperation()
                    guard response["ok"] as? Bool == true,
                          let result = response["value"] as? [String: Any] else {
                        message = response["error"] as? String ?? "配置预览失败；检查文件或密码后重试"
                        return
                    }
                    preview = result
                    selectedSections = Set((result["sections"] as? [[String: Any]] ?? [])
                        .compactMap { $0["id"] as? String })
                    message = "已校验配置；请选择需要替换的类别"
                }
            }
        } catch {
            tunnel.endConfigurationOperation()
            message = error.localizedDescription
        }
    }

    private func importConfiguration() {
        guard let url = importURL, let revision = preview?["expectedRevision"] as? String,
              !selectedSections.isEmpty else { return }
        guard tunnel.beginConfigurationOperation() else { return }
        let password = importPassword
        do {
            let directory = try dataDirectory()
            let sectionsData = try JSONSerialization.data(withJSONObject: selectedSections.sorted())
            guard let sectionsJSON = String(data: sectionsData, encoding: .utf8) else {
                throw NSError(domain: "HTTP Capture", code: 3,
                              userInfo: [NSLocalizedDescriptionKey: "无法编码导入类别"])
            }
            message = "正在导入配置…"
            DispatchQueue.global(qos: .userInitiated).async {
                let response = directory.path.withCString { path in
                    url.path.withCString { input in
                        sectionsJSON.withCString { sections in
                            revision.withCString { expected in
                                password.withCString { secret in
                                    Self.nativeResult(hc_ios_config_import(
                                        path, input, sections, expected, secret))
                                }
                            }
                        }
                    }
                }
                Task { @MainActor in
                    let imported = response["ok"] as? Bool == true
                    tunnel.endConfigurationOperation(imported: imported)
                    if imported {
                        preview = nil
                        selectedSections = []
                        message = "配置已导入，恢复点已保存。请完全关闭应用再重新打开，并手动启用需要的规则。"
                    } else {
                        message = response["error"] as? String ?? "配置导入失败"
                    }
                }
            }
        } catch {
            tunnel.endConfigurationOperation()
            message = error.localizedDescription
        }
    }

    private func sectionLabel(_ id: String) -> String {
        switch id {
        case "preferences": return "界面偏好"
        case "library": return "请求集合"
        case "workspace": return "桌面请求工作区"
        case "interception": return "拦截规则"
        case "blocking": return "阻止规则"
        case "faults": return "故障模拟规则"
        case "responseRewrite": return "响应修改规则"
        case "scripts": return "脚本"
        case "plugins": return "插件"
        case "decryption": return "解密排除域名"
        case "upstream": return "上游代理"
        case "mcp": return "MCP 客户端"
        default: return id
        }
    }
}
