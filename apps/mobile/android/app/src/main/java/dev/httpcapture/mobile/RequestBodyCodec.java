package dev.httpcapture.mobile;

import java.io.ByteArrayOutputStream;
import java.net.URLEncoder;
import java.nio.charset.StandardCharsets;

/** Byte encoders for the Android manual request editor. No Android APIs are needed. */
final class RequestBodyCodec {
    static final int MAX_BODY_BYTES = 1024 * 1024;

    private RequestBodyCodec() { }

    static byte[] urlForm(String fields) throws Exception {
        StringBuilder encoded = new StringBuilder();
        for (String line : fields.split("\\r?\\n")) {
            if (line.isEmpty()) continue;
            int separator = line.indexOf('=');
            if (separator <= 0) throw new IllegalArgumentException("表单每行须为 名称=值");
            if (encoded.length() > 0) encoded.append('&');
            encoded.append(URLEncoder.encode(line.substring(0, separator), "UTF-8"))
                .append('=')
                .append(URLEncoder.encode(line.substring(separator + 1), "UTF-8"));
            if (encoded.length() > MAX_BODY_BYTES) {
                throw new IllegalArgumentException("请求正文最多 1 MiB");
            }
        }
        return encoded.toString().getBytes(StandardCharsets.US_ASCII);
    }

    static byte[] multipart(String fields, String boundary, String fileFieldName, String fileName,
                            String mimeType, byte[] file) throws Exception {
        if (!boundary.matches("[A-Za-z0-9-]{1,70}")) {
            throw new IllegalArgumentException("Multipart 边界无效");
        }
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        for (String line : fields.split("\\r?\\n")) {
            if (line.isEmpty()) continue;
            int separator = line.indexOf('=');
            if (separator <= 0) throw new IllegalArgumentException("Multipart 字段每行须为 名称=值");
            String name = safeMultipartToken(line.substring(0, separator));
            writeUtf8(output, "--" + boundary + "\r\nContent-Disposition: form-data; name=\""
                + name + "\"\r\n\r\n" + line.substring(separator + 1) + "\r\n");
            checkSize(output.size());
        }
        if (file != null) {
            String fieldName = safeMultipartToken(fileFieldName == null ? "file" : fileFieldName);
            String name = safeMultipartToken(fileName == null ? "attachment.bin" : fileName);
            writeUtf8(output, "--" + boundary + "\r\nContent-Disposition: form-data; name=\""
                + fieldName + "\"; filename=\""
                + name + "\"\r\nContent-Type: " + safeMimeType(mimeType) + "\r\n\r\n");
            checkSize(output.size() + file.length);
            output.write(file);
            writeUtf8(output, "\r\n");
        }
        writeUtf8(output, "--" + boundary + "--\r\n");
        checkSize(output.size());
        return output.toByteArray();
    }

    static String safeMimeType(String value) {
        if (value == null || !value.matches("[A-Za-z0-9!#$&^_.+-]+/[A-Za-z0-9!#$&^_.+-]+")) {
            return "application/octet-stream";
        }
        return value;
    }

    private static String safeMultipartToken(String value) {
        for (int index = 0; index < value.length(); index++) {
            if (Character.isISOControl(value.charAt(index))) {
                throw new IllegalArgumentException("Multipart 字段名或文件名含控制字符");
            }
        }
        String cleaned = value.replace('"', '_').replace('\\', '_').replace('/', '_').trim();
        if (cleaned.isEmpty() || cleaned.length() > 128) {
            throw new IllegalArgumentException("Multipart 字段名或文件名无效");
        }
        return cleaned;
    }

    private static void writeUtf8(ByteArrayOutputStream output, String value) throws Exception {
        output.write(value.getBytes(StandardCharsets.UTF_8));
    }

    private static void checkSize(int size) {
        if (size > MAX_BODY_BYTES) throw new IllegalArgumentException("请求正文最多 1 MiB");
    }
}
