package dev.httpcapture.probe;

import java.net.InetSocketAddress;
import java.net.Proxy;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;

/** Host-JDK regression tests for readiness during Android VPN proxy-hint changes. */
public final class ProxyReadinessTest {
    private static final int PORT = 43873;
    private static int checks;

    public static void main(String[] args) {
        List<Proxy> direct = Collections.singletonList(Proxy.NO_PROXY);
        List<Proxy> local = Collections.singletonList(http("127.0.0.1", PORT));

        check(!ready(true, true, false, local),
                "framework cleared its proxy before the process selector; request must wait");
        check(ready(true, true, false, direct), "hint-off becomes ready after selector updates");
        check(!ready(false, true, false, direct), "direct proxy snapshots do not mean VPN is ready");
        check(!ready(false, true, true, local), "hint-on also waits for the probe UID's VPN");
        check(!ready(true, false, true, local), "selector must not outrun framework hint-on state");
        check(!ready(true, false, false, direct), "framework proxy must also be clear for hint-off");
        check(ready(true, true, true, local), "matching local HTTP proxy is ready for hint-on");

        check(!ready(true, true, true, Collections.singletonList(http("127.0.0.1", PORT + 1))),
                "wrong local proxy port");
        check(!ready(true, true, true, Collections.singletonList(http("localhost", PORT))),
                "hostname alias must not replace the expected literal loopback host");
        check(!ready(true, true, true, Collections.singletonList(http("192.168.1.2", PORT))),
                "wrong proxy host");
        check(!ready(true, true, true, Collections.singletonList(
                new Proxy(Proxy.Type.SOCKS, InetSocketAddress.createUnresolved("127.0.0.1", PORT)))),
                "SOCKS is not the advertised HTTP proxy");
        check(!ready(true, true, true, Arrays.asList(Proxy.NO_PROXY, local.get(0))),
                "hint-on must prefer the local HTTP proxy");
        check(ready(true, true, true, Arrays.asList(local.get(0), Proxy.NO_PROXY)),
                "hint-on accepts the advertised HTTP proxy as the first choice");
        check(!ready(true, true, false, Arrays.asList(Proxy.NO_PROXY, local.get(0))),
                "hint-off must reject any remaining HTTP proxy");
        check(!ready(true, true, false, Collections.singletonList(
                new Proxy(Proxy.Type.SOCKS, InetSocketAddress.createUnresolved("127.0.0.1", PORT)))),
                "other proxy types must not count as hint-off readiness");
        check(ready(true, true, false, Arrays.asList(Proxy.NO_PROXY, Proxy.NO_PROXY)),
                "all-DIRECT choices are ready");

        for (boolean expectHint : new boolean[] {false, true}) {
            check(!ready(true, true, expectHint, null), "missing selector snapshot is not ready");
            check(!ready(true, true, expectHint, Collections.emptyList()), "empty selector list is not ready");
            check(!ready(true, true, expectHint, Arrays.asList((Proxy) null)), "null proxy is not ready");
        }
        check(!ready(true, true, false, Arrays.asList(Proxy.NO_PROXY, null)),
                "mixed DIRECT and null list is not ready");
        check(!ProxyReadiness.isReady(true, true, true, 0, local), "invalid expected proxy port");
        check(!ProxyReadiness.isReady(true, true, true, 65536, local), "oversized expected proxy port");
        System.out.println("ProxyReadinessTest passed (" + checks + " checks)");
    }

    private static Proxy http(String host, int port) {
        return new Proxy(Proxy.Type.HTTP, InetSocketAddress.createUnresolved(host, port));
    }

    private static boolean ready(boolean vpn, boolean framework, boolean expectHint, List<Proxy> proxies) {
        return ProxyReadiness.isReady(vpn, framework, expectHint, PORT, proxies);
    }

    private static void check(boolean condition, String message) {
        checks++;
        if (!condition) throw new AssertionError(message);
    }
}
