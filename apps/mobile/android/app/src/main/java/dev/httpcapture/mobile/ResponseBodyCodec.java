package dev.httpcapture.mobile;

import java.util.Base64;
import java.util.Arrays;
import java.nio.charset.StandardCharsets;

/** Bounded decoding for binary response replacement bodies. */
final class ResponseBodyCodec {
    static final int MAX_BODY_BYTES = 64 * 1024;
    private static final int MAX_BASE64_CHARS = ((MAX_BODY_BYTES + 2) / 3) * 4;

    private ResponseBodyCodec() { }

    static byte[] decodeBase64(String input) {
        StringBuilder encoded = new StringBuilder(Math.min(input.length(), MAX_BASE64_CHARS));
        for (int index = 0; index < input.length(); index++) {
            char value = input.charAt(index);
            if (Character.isWhitespace(value)) continue;
            if (encoded.length() == MAX_BASE64_CHARS)
                throw new IllegalArgumentException("响应正文最多 64 KiB");
            encoded.append(value);
        }
        final byte[] bytes;
        try { bytes = Base64.getDecoder().decode(encoded.toString()); }
        catch (IllegalArgumentException error) {
            throw new IllegalArgumentException("正文不是有效的 Base64 编码", error);
        }
        if (bytes.length > MAX_BODY_BYTES)
            throw new IllegalArgumentException("响应正文最多 64 KiB");
        return bytes;
    }

    static boolean isReadableUtf8(byte[] bytes) {
        String text = new String(bytes, StandardCharsets.UTF_8);
        if (!Arrays.equals(bytes, text.getBytes(StandardCharsets.UTF_8))) return false;
        for (int index = 0; index < text.length(); index++) {
            char value = text.charAt(index);
            if (Character.isISOControl(value) && value != '\t' && value != '\n' && value != '\r')
                return false;
        }
        return true;
    }
}
