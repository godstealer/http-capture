package dev.httpcapture.mobile;

import java.io.ByteArrayOutputStream;
import java.io.DataInputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.Consumer;

import javax.net.ssl.SSLSocket;

/** Loopback-only HTTP CONNECT adapter for tun2proxy; every upstream socket is VPN-protected. */
final class ProtectedRelay implements AutoCloseable {
    private static final int UDP_GATEWAY_PORT = 65353;
    private final LanPairingClient.Session session;
    private final ServerSocket listener;
    private final ExecutorService workers = Executors.newCachedThreadPool();
    private final Set<Socket> peers = ConcurrentHashMap.newKeySet();
    private final AtomicBoolean closed = new AtomicBoolean();
    private final AtomicBoolean failed = new AtomicBoolean();
    private final Consumer<String> onFatal;

    ProtectedRelay(LanPairingClient.Session session, Consumer<String> onFatal) throws IOException {
        if (session == null || session.expiresAt <= System.currentTimeMillis())
            throw new IllegalStateException("PC 配对已过期，请重新扫码");
        this.session = session;
        this.onFatal = onFatal;
        this.listener = new ServerSocket(0, 64, InetAddress.getByName("127.0.0.1"));
        workers.execute(() -> {
            while (!closed.get()) {
                try {
                    Socket client = listener.accept();
                    peers.add(client);
                    workers.execute(() -> handle(client));
                } catch (IOException error) {
                    if (!closed.get()) fatal("本机转发入口已断开：" + error.getMessage());
                    return;
                }
            }
        });
        workers.execute(() -> {
            while (!closed.get()) {
                try {
                    Thread.sleep(5000);
                    if (!closed.get()) LanPairingClient.ping(session);
                } catch (InterruptedException stopped) { return; }
                catch (Exception error) {
                    fatal("PC 心跳失败：" + error.getMessage());
                    return;
                }
            }
        });
    }

    int port() { return listener.getLocalPort(); }
    int udpGatewayPort() { return UDP_GATEWAY_PORT; }

    private void handle(Socket client) {
        SSLSocket tls = null;
        boolean upstreamAttempted = false;
        try {
            client.setSoTimeout(8000);
            String head = readHead(client.getInputStream());
            String request = head.substring(0, head.indexOf("\r\n"));
            String[] pieces = request.split(" ", 3);
            if (pieces.length != 3 || !"CONNECT".equals(pieces[0]) || !pieces[2].startsWith("HTTP/1."))
                throw new IOException("只接受 VPN 内核的 CONNECT 请求");
            String authority = pieces[1];
            int separator = authority.lastIndexOf(':');
            if (separator <= 0) throw new IOException("目标地址无效");
            String host = authority.substring(0, separator);
            if (host.startsWith("[") && host.endsWith("]")) host = host.substring(1, host.length() - 1);
            byte[] hostname = host.getBytes(StandardCharsets.US_ASCII);
            int port = Integer.parseInt(authority.substring(separator + 1));
            if (hostname.length == 0 || hostname.length > 253 || port < 1 || port > 65535
                || !host.matches("[A-Za-z0-9.:-]+")) throw new IOException("目标地址无效");
            boolean udpGateway = host.equals("127.0.0.1") && port == UDP_GATEWAY_PORT;
            if (session.expiresAt <= System.currentTimeMillis()) throw new IOException("PC 会话已过期");

            upstreamAttempted = true;
            tls = LanPairingClient.connect(session.code, true);
            peers.add(tls);
            DataOutputStream upstream = new DataOutputStream(tls.getOutputStream());
            upstream.writeByte(udpGateway ? 4 : 2);
            upstream.write(session.token);
            if (!udpGateway) {
                upstream.writeShort(hostname.length);
                upstream.write(hostname);
                upstream.writeShort(port);
            }
            upstream.flush();
            int result = new DataInputStream(tls.getInputStream()).readUnsignedByte();
            if (result == 1) throw new IllegalStateException("PC 已撤销或拒绝会话");
            if (result != 0) {
                client.getOutputStream().write("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"
                    .getBytes(StandardCharsets.US_ASCII));
                return;
            }
            client.getOutputStream().write("HTTP/1.1 200 Connection Established\r\n\r\n"
                .getBytes(StandardCharsets.US_ASCII));
            client.setSoTimeout(0);
            tls.setSoTimeout(0);
            final SSLSocket remote = tls;
            workers.execute(() -> {
                try { copy(client.getInputStream(), remote.getOutputStream()); }
                catch (IOException ignored) { }
                finally { closeQuietly(client); closeQuietly(remote); }
            });
            copy(tls.getInputStream(), client.getOutputStream());
        } catch (Exception error) {
            if (upstreamAttempted && (tls == null || error instanceof IllegalStateException)) {
                fatal("PC 加密连接失效：" + error.getMessage());
            } else {
                try { client.getOutputStream().write("HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"
                    .getBytes(StandardCharsets.US_ASCII)); } catch (IOException ignored) { }
            }
        } finally {
            closeQuietly(client);
            closeQuietly(tls);
            peers.remove(client);
            if (tls != null) peers.remove(tls);
        }
    }

    private static String readHead(InputStream input) throws IOException {
        ByteArrayOutputStream head = new ByteArrayOutputStream();
        int matched = 0;
        byte[] terminator = {'\r', '\n', '\r', '\n'};
        while (head.size() < 8192) {
            int next = input.read();
            if (next < 0) throw new IOException("CONNECT 请求中断");
            head.write(next);
            matched = next == terminator[matched] ? matched + 1 : next == '\r' ? 1 : 0;
            if (matched == terminator.length) return head.toString("US-ASCII");
        }
        throw new IOException("CONNECT 请求头过长");
    }

    private static void copy(InputStream input, OutputStream output) throws IOException {
        byte[] bytes = new byte[16 * 1024];
        for (int count; (count = input.read(bytes)) >= 0;) {
            output.write(bytes, 0, count);
            output.flush();
        }
    }

    private void fatal(String reason) {
        if (failed.compareAndSet(false, true) && !closed.get()) onFatal.accept(reason);
    }

    private static void closeQuietly(Socket socket) {
        if (socket != null) try { socket.close(); } catch (IOException ignored) { }
    }

    @Override public void close() {
        if (!closed.compareAndSet(false, true)) return;
        try { listener.close(); } catch (IOException ignored) { }
        for (Socket peer : peers) closeQuietly(peer);
        workers.shutdownNow();
    }
}
