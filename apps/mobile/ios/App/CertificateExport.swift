import Foundation

enum CertificateExport {
    enum ExportError: LocalizedError {
        case invalidCertificate

        var errorDescription: String? {
            "抓包 CA 证书格式无效，无法导出供 iPhone 安装的证书文件"
        }
    }

    static func der(fromPEM pem: String) throws -> Data {
        let lines = pem.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
        guard lines.count >= 3,
              lines.first == "-----BEGIN CERTIFICATE-----",
              lines.last == "-----END CERTIFICATE-----" else {
            throw ExportError.invalidCertificate
        }
        let encoded = lines.dropFirst().dropLast().joined()
        guard let der = Data(base64Encoded: encoded), !der.isEmpty else {
            throw ExportError.invalidCertificate
        }
        return der
    }
}
