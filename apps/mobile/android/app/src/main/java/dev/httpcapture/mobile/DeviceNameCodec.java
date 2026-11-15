package dev.httpcapture.mobile;

import java.io.DataOutputStream;
import java.io.IOException;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/** Device descriptions and their authenticated wire frame, without Android dependencies. */
final class DeviceNameCodec {
    static final int MAX_NAME_BYTES = 128;

    private DeviceNameCodec() { }

    static String deviceName(String manufacturer, String model) {
        String brand = clean(manufacturer);
        String product = clean(model);
        String name;
        if (brand.isEmpty()) name = product;
        else if (product.isEmpty() || product.equalsIgnoreCase(brand)) name = brand;
        else if (product.regionMatches(true, 0, brand, 0, brand.length())
                && product.length() > brand.length()
                && isSpace(product.codePointAt(brand.length()))) name = product;
        else name = brand + " " + product;
        if (name.isEmpty()) name = "Android";

        int end = 0;
        int bytes = 0;
        while (end < name.length()) {
            int codePoint = name.codePointAt(end);
            int count = codePoint <= 0x7f ? 1 : codePoint <= 0x7ff ? 2 : codePoint <= 0xffff ? 3 : 4;
            if (bytes + count > MAX_NAME_BYTES) break;
            bytes += count;
            end += Character.charCount(codePoint);
        }
        return name.substring(0, end).trim();
    }

    static void writeReport(OutputStream output, byte[] token, String manufacturer, String model)
            throws IOException {
        if (token == null || token.length != 32)
            throw new IllegalArgumentException("A device report requires a 32-byte session token");
        byte[] name = deviceName(manufacturer, model).getBytes(StandardCharsets.UTF_8);
        DataOutputStream frame = new DataOutputStream(output);
        frame.writeByte(5);
        frame.write(token);
        frame.writeShort(name.length);
        frame.write(name);
        frame.flush();
    }

    private static String clean(String input) {
        if (input == null) return "";
        StringBuilder text = new StringBuilder();
        boolean space = false;
        for (int index = 0; index < input.length();) {
            int codePoint = input.codePointAt(index);
            index += Character.charCount(codePoint);
            if (Character.isISOControl(codePoint) || (codePoint >= 0xd800 && codePoint <= 0xdfff)
                    || codePoint == 0x061c || codePoint == 0x200e || codePoint == 0x200f
                    || codePoint == 0x2028 || codePoint == 0x2029
                    || (codePoint >= 0x202a && codePoint <= 0x202e)
                    || (codePoint >= 0x2066 && codePoint <= 0x2069)) continue;
            if (isSpace(codePoint)) {
                space = text.length() > 0;
            } else {
                if (space) text.append(' ');
                text.appendCodePoint(codePoint);
                space = false;
            }
        }
        String result = text.toString();
        return result.equalsIgnoreCase("unknown") ? "" : result;
    }

    private static boolean isSpace(int codePoint) {
        return Character.isWhitespace(codePoint) || Character.isSpaceChar(codePoint);
    }
}
