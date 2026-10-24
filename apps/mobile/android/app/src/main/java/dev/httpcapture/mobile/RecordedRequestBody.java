package dev.httpcapture.mobile;

import java.io.ByteArrayOutputStream;
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

    static long appendChunk(ByteArrayOutputStream output, long offset, long totalBytes,
                            byte[] chunk, Long nextOffset) {
        long end = offset + chunk.length;
        if (offset != output.size() || totalBytes < 0 || totalBytes > MAX_COPY_BYTES
                || chunk.length == 0 || chunk.length > 16 * 1024 || end > totalBytes
                || (end == totalBytes ? nextOffset != null
                    : nextOffset == null || nextOffset != end)) {
            throw new IllegalArgumentException("请求正文分段不完整或已变化，请重新打开详情");
        }
        output.write(chunk, 0, chunk.length);
        return end;
    }
}
