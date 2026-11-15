package dev.httpcapture.probe;

import java.net.InetSocketAddress;
import java.net.Proxy;
import java.util.List;

/** Checks a probe UID's network and both proxy snapshots without changing proxy selection. */
final class ProxyReadiness {
    private ProxyReadiness() { }

    static boolean isReady(boolean vpnReady, boolean frameworkReady, boolean expectProxyHint,
                           int proxyPort, List<Proxy> selectedProxies) {
        if (!vpnReady || !frameworkReady || proxyPort < 1 || proxyPort > 65535
                || selectedProxies == null || selectedProxies.isEmpty()) return false;
        if (expectProxyHint) {
            Proxy first = selectedProxies.get(0);
            if (first == null || first.type() != Proxy.Type.HTTP
                    || !(first.address() instanceof InetSocketAddress)) return false;
            InetSocketAddress address = (InetSocketAddress) first.address();
            return "127.0.0.1".equals(address.getHostString()) && address.getPort() == proxyPort;
        }
        for (Proxy proxy : selectedProxies)
            if (proxy == null || proxy.type() != Proxy.Type.DIRECT) return false;
        return true;
    }
}
