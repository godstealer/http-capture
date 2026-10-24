import SwiftUI
import UniformTypeIdentifiers

struct TrustedCertificatesView: View {
    @EnvironmentObject private var tunnel: TunnelController
    @Environment(\.dismiss) private var dismiss
    @State private var showingFilePicker = false
    @State private var name = ""
    @State private var host = ""
    @State private var port = "443"

    var body: some View {
        NavigationStack {
            Form {
                Section("导入目标服务器的 CA") {
                    TextField("证书名称", text: $name)
                    TextField("精确域名；留空适用于所有 HTTPS 目标", text: $host)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                    TextField("目标端口", text: $port).keyboardType(.numberPad)
                    Button("选择 PEM 或 DER 证书") { showingFilePicker = true }
                        .disabled(tunnel.trustedCertificatesBusy || name.trimmingCharacters(in: .whitespaces).isEmpty
                                  || (Int(port).map { !(1...65535).contains($0) } ?? true))
                    Text("只导入公开 CA 证书，不要选择私钥。导入后还需在下方启用；大于 64 KiB 的文件暂不支持。")
                        .font(.footnote).foregroundStyle(.secondary)
                }
                Section("已导入") {
                    if tunnel.trustedCertificates.isEmpty {
                        Text("暂无上游 CA").foregroundStyle(.secondary)
                    }
                    ForEach(tunnel.trustedCertificates) { certificate in
                        VStack(alignment: .leading, spacing: 5) {
                            Text(certificate.name).bold()
                            Text(certificate.host.isEmpty ? "所有 HTTPS 目标" : "\(certificate.host):\(certificate.port)")
                                .font(.footnote).foregroundStyle(.secondary)
                            HStack {
                                Button(certificate.enabled ? "停用" : "启用") {
                                    tunnel.changeTrustedCertificate(certificate)
                                }
                                Spacer()
                                Button("删除", role: .destructive) {
                                    tunnel.changeTrustedCertificate(certificate, delete: true)
                                }
                            }
                            .disabled(tunnel.trustedCertificatesBusy)
                        }
                    }
                }
                Text(tunnel.message).font(.footnote).foregroundStyle(.secondary)
            }
            .navigationTitle("上游 CA 证书")
            .toolbar { Button("关闭") { dismiss() } }
            .task { tunnel.refreshTrustedCertificates() }
            .fileImporter(isPresented: $showingFilePicker, allowedContentTypes: [.data]) { result in
                do {
                    let url = try result.get()
                    let scoped = url.startAccessingSecurityScopedResource()
                    defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                    if let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize, size > 64 * 1024 {
                        tunnel.reportTrustedCertificateError("证书文件超过 64 KiB")
                        return
                    }
                    let data = try Data(contentsOf: url)
                    guard let portNumber = Int(port) else { return }
                    tunnel.importTrustedCertificate(data: data, name: name, host: host, port: portNumber)
                } catch {
                    tunnel.reportTrustedCertificateError("读取证书失败：\(error.localizedDescription)")
                }
            }
        }
    }
}
