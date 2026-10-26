package dev.httpcapture.mobile;

import android.util.Base64;

import java.io.DataInputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.URI;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.SecureRandom;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.HashMap;
import java.util.List;
import java.util.Map;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509TrustManager;

/** The QR is only an identity hint and one-use secret; all redemption uses pinned TLS. */
final class LanPairingClient {
    private static volatile Session activeSession;

    static void setActiveSession(Session session) { activeSession = session; }
    static Session activeSession() { return activeSession; }
    static final class Code {
        final String host;
        final int port;
        final byte[] token;
        final byte[] fingerprint;
        final long expiresAt;

        Code(String host, int port, byte[] token, byte[] fingerprint, long expiresAt) {
            this.host = host; this.port = port; this.token = token;
            this.fingerprint = fingerprint; this.expiresAt = expiresAt;
        }

        String fingerprintText() {
            StringBuilder text = new StringBuilder();
            for (byte value : fingerprint) text.append(String.format("%02x", value & 0xff));
            return text.toString();
        }
    }

    static final class Session {
        final Code code;
        final byte[] token;
        final long expiresAt;
        Session(Code code, byte[] token) {
            this.code = code; this.token = token;
            this.expiresAt = System.currentTimeMillis() + 3_600_000L;
        }
    }

    static Code parse(String scanned) throws Exception {
        if (scanned == null || scanned.length() > 512) throw new IllegalArgumentException("配对码无效或过长");
        URI uri = new URI(scanned);
        if (!"httpcapture".equals(uri.getScheme()) || !"pair".equals(uri.getHost())
            || !"".equals(uri.getPath()) || uri.getFragment() != null)
            throw new IllegalArgumentException("不是 HTTP Capture 配对码");
        Map<String, String> fields = new HashMap<>();
        if (uri.getRawQuery() == null) throw new IllegalArgumentException("配对码缺少参数");
        for (String field : uri.getRawQuery().split("&", -1)) {
            int separator = field.indexOf('=');
            if (separator < 1) throw new IllegalArgumentException("配对码参数无效");
            String key = URLDecoder.decode(field.substring(0, separator), "UTF-8");
            String value = URLDecoder.decode(field.substring(separator + 1), "UTF-8");
            if (!(key.equals("v") || key.equals("host") || key.equals("port")
                || key.equals("token") || key.equals("fp") || key.equals("exp"))
                || fields.putIfAbsent(key, value) != null)
                throw new IllegalArgumentException("配对码参数重复或未知");
        }
        if (fields.size() != 6 || !"1".equals(fields.get("v")))
            throw new IllegalArgumentException("不支持的配对码版本");
        String host = fields.get("host");
        if (!privateIPv4(host)) throw new IllegalArgumentException("配对地址不是私有局域网 IPv4");
        int port = Integer.parseInt(fields.get("port"));
        if (port < 1024 || port > 65535) throw new IllegalArgumentException("配对端口无效");
        String encoded = fields.get("token");
        if (!encoded.matches("[A-Za-z0-9_-]{43}")) throw new IllegalArgumentException("配对凭据无效");
        byte[] token = Base64.decode(encoded, Base64.URL_SAFE | Base64.NO_PADDING | Base64.NO_WRAP);
        if (token.length != 32) throw new IllegalArgumentException("配对凭据长度无效");
        String hex = fields.get("fp");
        if (!hex.matches("[0-9a-fA-F]{64}")) throw new IllegalArgumentException("PC 身份指纹无效");
        byte[] fingerprint = new byte[32];
        for (int index = 0; index < fingerprint.length; index++)
            fingerprint[index] = (byte) Integer.parseInt(hex.substring(index * 2, index * 2 + 2), 16);
        long expiry = Long.parseLong(fields.get("exp"));
        long now = System.currentTimeMillis() / 1000;
        if (expiry <= now || expiry - now > 300) throw new IllegalArgumentException("配对码已过期或时间无效");
        return new Code(host, port, token, fingerprint, expiry);
    }

    static Session redeem(Code code) throws Exception {
        if (System.currentTimeMillis() / 1000 >= code.expiresAt)
            throw new IllegalArgumentException("配对码已过期");
        try (SSLSocket tls = connect(code, false)) {
            OutputStream output = tls.getOutputStream();
            output.write(1);
            output.write(code.token);
            output.flush();
            DataInputStream input = new DataInputStream(tls.getInputStream());
            if (input.readUnsignedByte() != 0) throw new IllegalStateException("PC 拒绝配对；请刷新二维码");
            byte[] session = new byte[32];
            input.readFully(session);
            return new Session(code, session);
        }
    }

    static void ping(Session session) throws Exception {
        if (session.expiresAt <= System.currentTimeMillis())
            throw new IllegalStateException("PC 会话已过期");
        try (SSLSocket tls = connect(session.code, true)) {
            OutputStream output = tls.getOutputStream();
            output.write(3);
            output.write(session.token);
            output.flush();
            if (new DataInputStream(tls.getInputStream()).readUnsignedByte() != 0)
                throw new IllegalStateException("PC 已撤销会话");
        }
    }

    static SSLSocket connect(Code code, boolean vpnMustBeActive) throws Exception {
        SSLContext context = SSLContext.getInstance("TLS");
        context.init(null, new TrustManager[] { new PinnedDesktop(code) }, new SecureRandom());
        Socket raw = new Socket();
        try {
            if ((vpnMustBeActive || CaptureVpnService.isActive())
                && !CaptureVpnService.protectRemoteSocket(raw))
                throw new IllegalStateException("无法排除 VPN 回环，已停止远程连接");
            raw.connect(new InetSocketAddress(code.host, code.port), 5000);
            SSLSocket tls = (SSLSocket) context.getSocketFactory()
                .createSocket(raw, code.host, code.port, true);
            tls.setSoTimeout(8000);
            tls.startHandshake();
            return tls;
        } catch (Exception error) {
            raw.close();
            throw error;
        }
    }

    private static boolean privateIPv4(String text) {
        if (text == null || !text.matches("[0-9]{1,3}(\\.[0-9]{1,3}){3}")) return false;
        String[] parts = text.split("\\.");
        int[] bytes = new int[4];
        for (int index = 0; index < 4; index++) {
            bytes[index] = Integer.parseInt(parts[index]);
            if (bytes[index] > 255 || !Integer.toString(bytes[index]).equals(parts[index])) return false;
        }
        return bytes[0] == 10 || (bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31)
            || (bytes[0] == 192 && bytes[1] == 168);
    }

    private static final class PinnedDesktop implements X509TrustManager {
        private final Code code;
        PinnedDesktop(Code code) { this.code = code; }
        @Override public void checkClientTrusted(X509Certificate[] chain, String authType)
            throws CertificateException { throw new CertificateException("Not a client trust manager"); }
        @Override public void checkServerTrusted(X509Certificate[] chain, String authType)
            throws CertificateException {
            if (chain == null || chain.length == 0) throw new CertificateException("PC 未提供身份凭据");
            X509Certificate certificate = chain[0];
            certificate.checkValidity();
            try {
                byte[] digest = MessageDigest.getInstance("SHA-256").digest(certificate.getPublicKey().getEncoded());
                if (!MessageDigest.isEqual(digest, code.fingerprint))
                    throw new CertificateException("PC 身份指纹与二维码不一致");
            } catch (CertificateException error) { throw error; }
            catch (Exception error) { throw new CertificateException("无法核对 PC 身份", error); }
            boolean addressMatches = false;
            try {
                for (List<?> alternate : certificate.getSubjectAlternativeNames()) {
                    if (alternate.size() >= 2 && Integer.valueOf(7).equals(alternate.get(0))
                        && code.host.equals(alternate.get(1))) addressMatches = true;
                }
            } catch (Exception error) { throw new CertificateException("无法核对 PC 地址", error); }
            if (!addressMatches) throw new CertificateException("PC 证书地址与二维码不一致");
        }
        @Override public X509Certificate[] getAcceptedIssuers() { return new X509Certificate[0]; }
    }
}
