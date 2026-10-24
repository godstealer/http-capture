package dev.httpcapture.mobile;

import java.nio.charset.StandardCharsets;
import java.util.Base64;

/** Lossless editor representation of a captured request body. */
final class RecordedRequestBody {
    static final int MAX_COPY_BYTES = 64 * 1024;
    final String value;
    final int mode;

    private RecordedRequestBody(String value, int mode) {
        this.value = value;
        this.mode = mode;
    }

    static RecordedRequestBody from(byte[] bytes) {
        if (bytes.length > MAX_COPY_BYTES)
            throw new IllegalArgumentException("请求正文超过 64 KiB，当前移动编辑器尚不能安全编辑后重发");
        if (ResponseBodyCodec.isReadableUtf8(bytes))
            return new RecordedRequestBody(new String(bytes, StandardCharsets.UTF_8), 0);
        return new RecordedRequestBody(Base64.getEncoder().encodeToString(bytes), 5);
    }
}
