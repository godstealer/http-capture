package dev.httpcapture.probe;

import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.os.SystemClock;
import android.util.Base64;

import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.InputStream;
import java.net.DatagramPacket;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Proxy;
import java.net.ProxySelector;
import java.net.Socket;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.util.Arrays;
import java.util.List;

/** Raw sockets from a separate UID; no direct tunnel operations or proxy overrides. */
final class LanProbe {
    static final String ACTION = "dev.httpcapture.probe.RUN_LAN";
    static final String RESULT = "dev.httpcapture.mobile.LAN_PROBE_RESULT";
    private static final int TIMEOUT = 5000;

    private LanProbe() { }

    static void run(Context context, Intent request) {
        String runId = "";
        String stage = "config", errorType = "";
        boolean vpnReady = false, noProxyHint = false;
        boolean httpOk = false, udpOk = false, dnsWireOk = false, virtualDnsOk = false;
        try {
            runId = request.getStringExtra("runId");
            if (runId == null || !runId.matches("[0-9a-f]{32}")) throw new IllegalArgumentException();
            String host = request.getStringExtra("host");
            if (!privateIPv4(host)) throw new IllegalArgumentException();
            int httpPort = checkedPort(request.getIntExtra("httpPort", 0));
            int udpPort = checkedPort(request.getIntExtra("udpPort", 0));
            int dnsWirePort = checkedPort(request.getIntExtra("dnsWirePort", 0));
            byte[] requestBody = body(request.getStringExtra("expectedRequestBodyBase64"));
            byte[] responseBody = body(request.getStringExtra("expectedResponseBodyBase64"));
            stage = "uid-vpn";
            awaitVpnWithoutProxy(context, new URI("http://" + host + ":" + httpPort + "/"));
            vpnReady = true;
            noProxyHint = true;
            stage = "http";
            http(host, httpPort, runId, requestBody, responseBody);
            httpOk = true;
            stage = "udp";
            udp(host, udpPort, runId);
            udpOk = true;
            // A high-port DNS datagram is ordinary remote UDP. It does not test
            // Android's system resolver or claim PC-side system DNS forwarding.
            stage = "dns-wire";
            dns(host, dnsWirePort, runId, true);
            dnsWireOk = true;
            // Production uses Virtual DNS: this answer must come from its own
            // fake-address pool, independently of the PC DNS-wire fixture.
            stage = "virtual-dns";
            dns("10.111.0.1", 53, runId, false);
            virtualDnsOk = true;
            stage = "done";
        } catch (Throwable failure) {
            errorType = fixedErrorType(failure.getClass().getSimpleName());
            if (runId == null || !runId.matches("[0-9a-f]{32}")) runId = "";
            // Do not return exception messages, addresses or arbitrary extras.
        } finally {
            Intent result = new Intent(RESULT).setPackage("dev.httpcapture.mobile");
            result.putExtra("runId", runId);
            result.putExtra("stage", stage);
            result.putExtra("errorType", errorType);
            result.putExtra("vpnReady", vpnReady);
            result.putExtra("noProxyHint", noProxyHint);
            result.putExtra("httpOk", httpOk);
            result.putExtra("udpOk", udpOk);
            result.putExtra("dnsWireOk", dnsWireOk);
            result.putExtra("virtualDnsOk", virtualDnsOk);
            result.putExtra("ok", vpnReady && noProxyHint && httpOk && udpOk && dnsWireOk && virtualDnsOk);
            context.sendBroadcast(result);
        }
    }

    private static void awaitVpnWithoutProxy(Context context, URI target) throws Exception {
        ConnectivityManager manager = context.getSystemService(ConnectivityManager.class);
        if (manager == null) throw new IllegalStateException();
        long deadline = SystemClock.elapsedRealtime() + 8000;
        while (SystemClock.elapsedRealtime() < deadline) {
            Network network = manager.getActiveNetwork();
            NetworkCapabilities capabilities = network == null ? null : manager.getNetworkCapabilities(network);
            boolean vpn = capabilities != null && capabilities.hasTransport(NetworkCapabilities.TRANSPORT_VPN);
            boolean direct = manager.getDefaultProxy() == null;
            ProxySelector selector = ProxySelector.getDefault();
            List<Proxy> proxies = selector == null ? null : selector.select(target);
            direct &= proxies != null && !proxies.isEmpty();
            if (proxies != null) for (Proxy proxy : proxies)
                direct &= proxy != null && proxy.type() == Proxy.Type.DIRECT;
            if (vpn && direct) return;
            Thread.sleep(100);
        }
        throw new IllegalStateException();
    }

    private static void http(String host, int port, String runId, byte[] requestBody, byte[] responseBody) throws Exception {
        try (Socket socket = new Socket()) {
            socket.connect(new InetSocketAddress(InetAddress.getByName(host), port), TIMEOUT);
            socket.setSoTimeout(TIMEOUT);
            String fields = "Host: " + host + ":" + port + "\r\nX-Order: one\r\nx-order: two\r\n";
            socket.getOutputStream().write(("POST /lan-" + runId + "/one HTTP/1.1\r\n"
                    + fields + "Content-Length: " + requestBody.length + "\r\n\r\n").getBytes(StandardCharsets.US_ASCII));
            socket.getOutputStream().write(requestBody);
            socket.getOutputStream().flush();
            InputStream input = socket.getInputStream();
            String head = readHead(input);
            String expected = "HTTP/1.1 200 OK\r\nX-Order: one\r\nx-order: two\r\nContent-Length: " + responseBody.length + "\r\n\r\n";
            if (!expected.equals(head) || !Arrays.equals(responseBody, readExactly(input, responseBody.length)))
                throw new IllegalStateException();
            socket.getOutputStream().write(("GET /lan-" + runId + "/two HTTP/1.1\r\n" + fields + "\r\n").getBytes(StandardCharsets.US_ASCII));
            socket.getOutputStream().flush();
            if (!"HTTP/1.1 204 No Content\r\nX-Order: one\r\nx-order: two\r\n\r\n".equals(readHead(input)))
                throw new IllegalStateException();
            // Close only after both framed replies were fully read. ProtectedRelay
            // currently closes both directions on EOF; early half-close is not tested.
        }
    }

    private static void udp(String host, int port, String runId) throws Exception {
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(InetAddress.getByName(host), port);
            socket.setSoTimeout(TIMEOUT);
            for (int index = 1; index <= 2; index++) {
                byte[] marker = ("lan-udp-" + runId + "-" + index).getBytes(StandardCharsets.US_ASCII);
                byte[] payload = Arrays.copyOf(marker, marker.length + 2);
                payload[marker.length] = 0;
                payload[marker.length + 1] = (byte) (index == 1 ? 0xff : 0x80);
                socket.send(new DatagramPacket(payload, payload.length));
                byte[] buffer = new byte[2048];
                DatagramPacket reply = new DatagramPacket(buffer, buffer.length);
                socket.receive(reply);
                if (!Arrays.equals(payload, Arrays.copyOfRange(reply.getData(), reply.getOffset(), reply.getOffset() + reply.getLength())))
                    throw new IllegalStateException();
            }
        }
    }

    private static void dns(String host, int port, String runId, boolean wireFixture) throws Exception {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        bytes.write(new byte[] {0x4c, 0x41, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0});
        for (String label : ("lan-" + runId + ".invalid").split("\\.")) {
            byte[] value = label.getBytes(StandardCharsets.US_ASCII);
            bytes.write(value.length);
            bytes.write(value);
        }
        bytes.write(new byte[] {0, 0, 1, 0, 1}); // A / IN.
        byte[] query = bytes.toByteArray();
        try (DatagramSocket socket = new DatagramSocket()) {
            socket.connect(InetAddress.getByName(host), port);
            socket.setSoTimeout(TIMEOUT);
            socket.send(new DatagramPacket(query, query.length));
            byte[] buffer = new byte[2048];
            DatagramPacket packet = new DatagramPacket(buffer, buffer.length);
            socket.receive(packet);
            byte[] reply = Arrays.copyOfRange(packet.getData(), packet.getOffset(), packet.getOffset() + packet.getLength());
            if (reply.length < query.length || reply[0] != query[0] || reply[1] != query[1]
                    || (reply[2] & 0x80) == 0 || (reply[2] & 0x02) != 0 || (reply[3] & 0x0f) != 0
                    || number(reply, 4) != 1 || number(reply, 6) < 1
                    || !Arrays.equals(Arrays.copyOfRange(reply, 12, query.length), Arrays.copyOfRange(query, 12, query.length)))
                throw new IllegalStateException();
            int offset = query.length;
            boolean found = false;
            for (int index = 0; index < number(reply, 6); index++) {
                offset = skipName(reply, offset);
                if (offset + 10 > reply.length) throw new IllegalStateException();
                int type = number(reply, offset), dnsClass = number(reply, offset + 2), size = number(reply, offset + 8);
                offset += 10;
                if (offset + size > reply.length) throw new IllegalStateException();
                if (type == 1 && dnsClass == 1 && size == 4) {
                    if (wireFixture) found |= Arrays.equals(Arrays.copyOfRange(reply, offset, offset + 4), new byte[] {(byte) 192, 0, 2, 1});
                    else found |= (reply[offset] & 0xff) == 198 && ((reply[offset + 1] & 0xff) == 18 || (reply[offset + 1] & 0xff) == 19);
                }
                offset += size;
            }
            if (!found) throw new IllegalStateException();
        }
    }

    private static int skipName(byte[] bytes, int offset) {
        for (int labels = 0; labels < 128; labels++) {
            if (offset >= bytes.length) throw new IllegalStateException();
            int size = bytes[offset++] & 0xff;
            if (size == 0) return offset;
            if ((size & 0xc0) == 0xc0) {
                if (offset >= bytes.length || (((size & 0x3f) << 8) | (bytes[offset] & 0xff)) >= bytes.length)
                    throw new IllegalStateException();
                return offset + 1;
            }
            if (size > 63 || offset + size > bytes.length) throw new IllegalStateException();
            offset += size;
        }
        throw new IllegalStateException();
    }
    private static int number(byte[] bytes, int offset) {
        if (offset + 2 > bytes.length) throw new IllegalStateException();
        return ((bytes[offset] & 0xff) << 8) | (bytes[offset + 1] & 0xff);
    }
    private static String readHead(InputStream input) throws Exception {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        int matched = 0;
        byte[] end = {'\r', '\n', '\r', '\n'};
        while (output.size() < 16384) {
            int next = input.read();
            if (next < 0) throw new EOFException();
            output.write(next);
            matched = next == end[matched] ? matched + 1 : next == '\r' ? 1 : 0;
            if (matched == end.length) return output.toString("ISO-8859-1");
        }
        throw new IllegalStateException();
    }
    private static byte[] readExactly(InputStream input, int count) throws Exception {
        byte[] bytes = new byte[count];
        int offset = 0;
        while (offset < count) {
            int read = input.read(bytes, offset, count - offset);
            if (read < 0) throw new EOFException();
            offset += read;
        }
        return bytes;
    }
    private static byte[] body(String value) {
        if (value == null || value.length() > 90000) throw new IllegalArgumentException();
        byte[] bytes = Base64.decode(value, Base64.DEFAULT);
        if (bytes.length > 65536) throw new IllegalArgumentException();
        return bytes;
    }
    private static String fixedErrorType(String type) {
        for (String allowed : new String[] {"SocketTimeoutException", "ConnectException", "SocketException", "IllegalStateException", "IllegalArgumentException", "EOFException", "SecurityException", "InterruptedException", "IOException"})
            if (allowed.equals(type)) return allowed;
        return "ProbeFailure";
    }
    private static int checkedPort(int port) {
        if (port < 1024 || port > 65535) throw new IllegalArgumentException();
        return port;
    }
    private static boolean privateIPv4(String value) {
        if (value == null || !value.matches("[0-9]{1,3}(\\.[0-9]{1,3}){3}")) return false;
        String[] fields = value.split("\\.");
        int[] bytes = new int[4];
        for (int index = 0; index < 4; index++) {
            bytes[index] = Integer.parseInt(fields[index]);
            if (bytes[index] > 255 || !Integer.toString(bytes[index]).equals(fields[index])) return false;
        }
        return bytes[0] == 10 || (bytes[0] == 172 && bytes[1] >= 16 && bytes[1] <= 31) || (bytes[0] == 192 && bytes[1] == 168);
    }
}
