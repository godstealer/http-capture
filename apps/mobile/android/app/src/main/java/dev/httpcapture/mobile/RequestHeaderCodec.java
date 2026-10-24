package dev.httpcapture.mobile;

import java.util.ArrayList;
import java.util.List;

/** Converts the mobile bulk header editor to ordered transport headers. */
final class RequestHeaderCodec {
    static final class Header {
        final String name;
        final String value;
        final boolean disabled;

        Header(String name, String value, boolean disabled) {
            this.name = name;
            this.value = value;
            this.disabled = disabled;
        }
    }

    private RequestHeaderCodec() { }

    static List<Header> parse(String text) {
        return parse(text, true);
    }

    static List<Header> parseResponse(String text) {
        return parse(text, false);
    }

    private static List<Header> parse(String text, boolean ignoreBrowserRequestMetadata) {
        List<Header> rows = new ArrayList<>();
        String kind = ignoreBrowserRequestMetadata ? "请求" : "响应";
        String[] lines = text.split("\\r?\\n", -1);
        for (int i = 0; i < lines.length; i++) {
            String line = lines[i].trim();
            if (line.isEmpty()) continue;
            boolean disabled = line.startsWith("# ");
            String content = disabled ? line.substring(2) : line;
            if (ignoreBrowserRequestMetadata && (isRequestLine(content) || isPseudoHeader(content))) continue;
            int colon = content.indexOf(':');
            if (colon <= 0) throw new IllegalArgumentException("第 " + (i + 1) + " 行" + kind + "头无效，请按 名称: 值 填写");
            String name = content.substring(0, colon).trim();
            String value = content.substring(colon + 1).trim();
            if (name.isEmpty() || !isToken(name) || hasBadValueCharacter(value)) {
                throw new IllegalArgumentException("第 " + (i + 1) + " 行" + kind + "头无效，请按 名称: 值 填写");
            }
            rows.add(new Header(name, value, disabled));
        }
        return rows;
    }

    static boolean isHeaderLine(String line) {
        try { return parse(line).size() == 1; }
        catch (IllegalArgumentException ignored) { return false; }
    }

    static String toggleLine(String text, int index) {
        String[] lines = text.split("\\n", -1);
        if (index < 0 || index >= lines.length || !isHeaderLine(lines[index])) return text;
        String line = lines[index].trim();
        lines[index] = line.startsWith("# ") ? line.substring(2) : "# " + line;
        return String.join("\n", lines);
    }

    private static boolean isRequestLine(String line) {
        String[] parts = line.split("\\s+");
        if (parts.length < 3 || !parts[parts.length - 1].startsWith("HTTP/")) return false;
        switch (parts[0]) {
            case "GET": case "POST": case "PUT": case "PATCH": case "DELETE":
            case "HEAD": case "OPTIONS": case "CONNECT": case "TRACE": return true;
            default: return false;
        }
    }

    private static boolean isPseudoHeader(String line) {
        return line.startsWith(":") && line.indexOf(':', 1) > 1;
    }

    private static boolean isToken(String name) {
        for (int i = 0; i < name.length(); i++) {
            char c = name.charAt(i);
            if (c >= '0' && c <= '9' || c >= 'A' && c <= 'Z' || c >= 'a' && c <= 'z') continue;
            if ("!#$%&'*+-.^_`|~".indexOf(c) < 0) return false;
        }
        return true;
    }

    private static boolean hasBadValueCharacter(String value) {
        for (int i = 0; i < value.length(); i++) {
            char c = value.charAt(i);
            if (c < 0x20 && c != '\t' || c == 0x7F) return true;
        }
        return false;
    }
}
