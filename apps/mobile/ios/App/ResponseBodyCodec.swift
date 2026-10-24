import Foundation

enum ResponseBodyCodec {
    static let maxBodyBytes = 64 * 1024
    private static let maxBase64Characters = ((maxBodyBytes + 2) / 3) * 4

    enum BodyError: Error {
        case invalid(String)
    }

    static func decodeBase64(_ input: String) throws -> Data {
        let encoded = input.filter { !$0.isWhitespace }
        guard encoded.utf8.count <= maxBase64Characters,
              let bytes = Data(base64Encoded: encoded) else {
            throw BodyError.invalid("正文不是有效的 Base64 编码，最多对应 64 KiB 原始字节")
        }
        guard bytes.count <= maxBodyBytes else {
            throw BodyError.invalid("响应正文最多 64 KiB")
        }
        return bytes
    }

    static func readableText(_ bytes: Data) -> String? {
        guard let text = String(data: bytes, encoding: .utf8),
              text.unicodeScalars.allSatisfy({ scalar in
                  !CharacterSet.controlCharacters.contains(scalar)
                      || scalar.value == 9 || scalar.value == 10 || scalar.value == 13
              }) else { return nil }
        return text
    }
}
