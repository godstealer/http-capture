import Foundation

func expect(_ condition: @autoclosure () -> Bool, _ reason: String) {
    precondition(condition(), reason)
}

do {
    let form = try RequestBodyCodec.compose(mode: .form, text: "城市=台北 桃园\nx=a+b", file: nil,
                                            fileName: nil, mimeType: nil)
    expect(String(data: form.bytes, encoding: .utf8) ==
           "%E5%9F%8E%E5%B8%82=%E5%8F%B0%E5%8C%97+%E6%A1%83%E5%9B%AD&x=a%2Bb",
           "URL 表单编码错误")
    expect(form.contentType == "application/x-www-form-urlencoded", "URL 表单 Content-Type 错误")

    let json = try RequestBodyCodec.compose(mode: .json, text: "{\"ok\":true}", file: nil,
                                            fileName: nil, mimeType: nil)
    expect(json.bytes == Data("{\"ok\":true}".utf8), "JSON 正文被改动")
    expect(json.contentType == "application/json; charset=utf-8", "JSON Content-Type 错误")
    do {
        _ = try RequestBodyCodec.compose(mode: .json, text: "{broken", file: nil,
                                         fileName: nil, mimeType: nil)
        preconditionFailure("无效 JSON 应被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }

    let binary = Data([0, 255, 10])
    let multipart = try RequestBodyCodec.compose(mode: .multipart, text: "city=台北", file: binary,
                                                 fileName: "test\".bin", mimeType: "bad\r\ntype")
    let boundary = String(multipart.contentType!.split(separator: "=").last!)
    expect(multipart.replaceContentType, "Multipart 必须更新旧 Content-Type")
    expect(multipart.bytes.starts(with: Data("--\(boundary)\r\n".utf8)), "Multipart 起始边界错误")
    expect(multipart.bytes.range(of: Data("name=\"file\"; filename=\"test_.bin\"".utf8)) != nil,
           "文件名没有安全编码")
    expect(multipart.bytes.range(of: Data("Content-Type: application/octet-stream".utf8)) != nil,
           "非法 MIME 未回退")
    expect(multipart.bytes.range(of: binary) != nil, "文件原始字节丢失")
    expect(Data(multipart.bytes.suffix(Data("--\(boundary)--\r\n".utf8).count)) == Data("--\(boundary)--\r\n".utf8),
           "Multipart 结束边界错误")

    let raw = try RequestBodyCodec.compose(mode: .base64, text: "AP8K\n", file: nil,
                                           fileName: nil, mimeType: nil)
    expect(raw.bytes == binary, "Base64 解码错误")
    do {
        _ = try RequestBodyCodec.compose(mode: .file, text: "", file: Data(count: 64 * 1024 + 1),
                                         fileName: "large", mimeType: nil)
        preconditionFailure("超大文件应被拒绝")
    } catch RequestBodyCodec.BodyError.invalid(_) { }
    print("iOS request body codec: PASS")
} catch {
    fatalError(error.localizedDescription)
}
