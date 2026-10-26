package dev.httpcapture.probe;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.ProxyInfo;
import android.util.Log;
import android.os.Process;

import java.io.ByteArrayInputStream;
import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.HttpURLConnection;
import java.net.Socket;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.security.KeyStore;
import java.security.cert.CertificateFactory;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.TrustManagerFactory;

/** Separate-UID raw TCP client used only by the emulator VPN smoke test. */
public final class ProbeReceiver extends BroadcastReceiver {
    @Override public void onReceive(Context context, Intent intent) {
        PendingResult pending = goAsync();
        new Thread(() -> {
            try {
                if ("dev.httpcapture.probe.RUN_TLS".equals(intent.getAction())) {
                    runTls(intent.getStringExtra("caPem"));
                } else if ("dev.httpcapture.probe.RUN_UDP".equals(intent.getAction())) {
                    runUdp();
                } else if ("dev.httpcapture.probe.RUN_DNS_V6".equals(intent.getAction())) {
                    runDnsV6();
                    sendDnsV6Result(context, null);
                } else if ("dev.httpcapture.probe.RUN_HTTP_STACK".equals(intent.getAction())) {
                    runHttpStack(context, intent);
                } else {
                    runHttp(context, intent.getStringExtra("path"));
                }
            } catch (Exception error) {
                Log.e("HttpCaptureProbe", "raw TCP request failed", error);
                if ("dev.httpcapture.probe.RUN_DNS_V6".equals(intent.getAction()))
                    sendDnsV6Result(context, error.toString());
            } finally {
                pending.finish();
            }
        }, "vpn-probe-client").start();
    }

    private static void runHttp(Context context, String path) throws Exception {
        if (path == null) path = "/vpn-smoke";
        ConnectivityManager connectivity = context.getSystemService(ConnectivityManager.class);
        Log.i("HttpCaptureProbe", "raw TCP active network: "
                + (connectivity == null ? "unavailable" : connectivity.getActiveNetwork())
                + " / " + (connectivity == null ? "unavailable" : connectivity.getNetworkCapabilities(
                        connectivity.getActiveNetwork())));
        try (Socket socket = new Socket("10.0.2.2", 18080)) {
            Log.i("HttpCaptureProbe", "raw TCP local address: " + socket.getLocalSocketAddress());
            socket.setSoTimeout(10_000);
            socket.getOutputStream().write(("GET " + path + " HTTP/1.1\r\n"
                    + "Host: 10.0.2.2:18080\r\nX-VPN-Probe-UID: " + Process.myUid()
                    + "\r\nConnection: close\r\n\r\n")
                    .getBytes(StandardCharsets.US_ASCII));
            InputStream response = socket.getInputStream();
            if (response.read() != 'H') throw new IllegalStateException("origin did not respond with HTTP");
            Log.i("HttpCaptureProbe", "raw TCP request reached origin");
        }
    }

    private static void runHttpStack(Context context, Intent intent) throws Exception {
        String path = intent.getStringExtra("path");
        if (path == null) throw new IllegalArgumentException("missing HTTP path");
        boolean expectProxyHint = intent.getBooleanExtra("expectProxyHint", false);
        int proxyPort = intent.getIntExtra("proxyPort", 0);
        ConnectivityManager connectivity = context.getSystemService(ConnectivityManager.class);
        if (connectivity == null) throw new IllegalStateException("no ConnectivityManager");
        long deadline = System.currentTimeMillis() + 5_000;
        boolean matches;
        do {
            ProxyInfo proxy = connectivity.getDefaultProxy();
            boolean localHint = proxy != null && "127.0.0.1".equals(proxy.getHost())
                    && proxy.getPort() == proxyPort;
            matches = localHint == expectProxyHint;
            if (!matches) Thread.sleep(100);
        } while (!matches && System.currentTimeMillis() < deadline);
        if (!matches) throw new IllegalStateException("VPN HTTP proxy hint did not match expected "
                + expectProxyHint);
        Log.i("HttpCaptureProbe", "VPN HTTP proxy hint observed: " + expectProxyHint);
        HttpURLConnection connection = (HttpURLConnection) new URL(
                "http://10.0.2.2:18080" + path).openConnection();
        connection.setConnectTimeout(10_000);
        connection.setReadTimeout(10_000);
        connection.setRequestProperty("X-VPN-Probe-UID", Integer.toString(Process.myUid()));
        try {
            if (connection.getResponseCode() != 200)
                throw new IllegalStateException("HTTP stack request returned "
                        + connection.getResponseCode());
            try (InputStream body = connection.getInputStream()) {
                if (body.read() < 0) throw new IllegalStateException("empty HTTP response");
            }
            Log.i("HttpCaptureProbe", "HTTP stack request succeeded: " + path);
        } finally {
            connection.disconnect();
        }
    }

    private static void runUdp() throws Exception {
        byte[] marker = "vpn-udp-smoke-01".getBytes(StandardCharsets.US_ASCII);
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(InetAddress.getByName("10.0.2.2"), 18082);
            socket.setSoTimeout(10_000);
            socket.send(new DatagramPacket(marker, marker.length));
            byte[] reply = new byte[64];
            DatagramPacket packet = new DatagramPacket(reply, reply.length);
            socket.receive(packet);
            if (!Arrays.equals(marker, Arrays.copyOfRange(packet.getData(),
                    packet.getOffset(), packet.getOffset() + packet.getLength()))) {
                throw new IllegalStateException("VPN UDP echo changed payload");
            }
            Log.i("HttpCaptureProbe", "raw VPN UDP echo succeeded");
        }
    }

    private static void runDnsV6() throws Exception {
        String name = "vpn-smoke.invalid";
        byte[] query = new byte[12 + name.length() + 2 + 4];
        query[0] = (byte) 0xCA;
        query[1] = (byte) 0xFE;
        query[2] = 1; // Recursion desired.
        query[5] = 1; // One question.
        int offset = 12;
        for (String label : name.split("\\.")) {
            byte[] bytes = label.getBytes(StandardCharsets.US_ASCII);
            query[offset++] = (byte) bytes.length;
            System.arraycopy(bytes, 0, query, offset, bytes.length);
            offset += bytes.length;
        }
        query[offset++] = 0;
        query[offset++] = 0;
        query[offset++] = 28; // AAAA.
        query[offset++] = 0;
        query[offset++] = 1; // IN.
        if (offset != query.length) throw new IllegalStateException("invalid DNS query length");
        try (DatagramSocket socket = new DatagramSocket(null)) {
            socket.bind(new InetSocketAddress(InetAddress.getByName("::"), 0));
            socket.connect(InetAddress.getByName("fd00:198:18::1"), 53);
            socket.setSoTimeout(10_000);
            socket.send(new DatagramPacket(query, query.length));
            byte[] buffer = new byte[512];
            DatagramPacket reply = new DatagramPacket(buffer, buffer.length);
            socket.receive(reply);
            int size = reply.getLength();
            if (size < query.length + 16 || buffer[0] != query[0] || buffer[1] != query[1]
                    || (buffer[2] & 0x80) == 0 || (buffer[3] & 0x0f) != 0
                    || (((buffer[6] & 0xff) << 8) | (buffer[7] & 0xff)) == 0)
                throw new IllegalStateException("virtual IPv6 DNS did not answer AAAA query");
            byte[] answer = Arrays.copyOfRange(buffer, size - 16, size);
            byte[] expectedPrefix = Arrays.copyOfRange(
                InetAddress.getByName("fd00:198:19::").getAddress(), 0, 12);
            if (!Arrays.equals(expectedPrefix, Arrays.copyOfRange(answer, 0, 12)))
                throw new IllegalStateException("virtual IPv6 DNS returned wrong AAAA pool");
            Log.i("HttpCaptureProbe", "VPN IPv6 virtual DNS AAAA succeeded");
        }
    }

    private static void sendDnsV6Result(Context context, String error) {
        Intent result = new Intent("dev.httpcapture.mobile.DNS_V6_RESULT")
            .setPackage("dev.httpcapture.mobile");
        result.putExtra("error", error);
        context.sendBroadcast(result);
    }

    private static void runTls(String caPem) throws Exception {
        if (caPem == null || !caPem.contains("BEGIN CERTIFICATE")) {
            throw new IllegalArgumentException("missing capture CA");
        }
        KeyStore roots = KeyStore.getInstance(KeyStore.getDefaultType());
        roots.load(null);
        roots.setCertificateEntry("capture-test-ca", CertificateFactory.getInstance("X.509")
                .generateCertificate(new ByteArrayInputStream(caPem.getBytes(StandardCharsets.US_ASCII))));
        TrustManagerFactory trust = TrustManagerFactory.getInstance(
                TrustManagerFactory.getDefaultAlgorithm());
        trust.init(roots);
        SSLContext tls = SSLContext.getInstance("TLS");
        tls.init(null, trust.getTrustManagers(), null);
        try (SSLSocket socket = (SSLSocket) tls.getSocketFactory()
                .createSocket("10.0.2.2", 18081)) {
            socket.setSoTimeout(15_000);
            SSLParameters params = socket.getSSLParameters();
            params.setEndpointIdentificationAlgorithm("HTTPS");
            socket.setSSLParameters(params);
            socket.startHandshake();
            socket.getOutputStream().write(("GET /vpn-tls-smoke HTTP/1.1\r\n"
                    + "Host: 10.0.2.2:18081\r\nConnection: close\r\n\r\n")
                    .getBytes(StandardCharsets.US_ASCII));
            InputStream response = socket.getInputStream();
            byte[] head = new byte[12];
            int length = 0;
            while (length < head.length) {
                int count = response.read(head, length, head.length - length);
                if (count == -1) break;
                length += count;
            }
            if (length != head.length || !new String(head, StandardCharsets.US_ASCII)
                    .startsWith("HTTP/1.1 403")) {
                throw new IllegalStateException("intercepted TLS request was not blocked");
            }
            Log.i("HttpCaptureProbe", "raw VPN TLS request was intercepted");
        }
    }
}
