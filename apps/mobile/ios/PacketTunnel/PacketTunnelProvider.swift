import Foundation
import NetworkExtension
import Darwin
import Dispatch

final class PacketTunnelProvider: NEPacketTunnelProvider {
    private let lifecycleLock = NSLock()
    private var isRunning = false
    private var isStarting = false
    private var isStopping = false
    private var generation: UInt64 = 0
    private let healthQueue = DispatchQueue(label: "dev.httpcapture.mobile.tunnel-health")
    private var healthTimer: DispatchSourceTimer?

    private var running: Bool {
        lifecycleLock.lock()
        defer { lifecycleLock.unlock() }
        return isRunning
    }

    override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
        guard let configuration = protocolConfiguration as? NETunnelProviderProtocol,
              let number = configuration.providerConfiguration?["port"] as? NSNumber,
              (1...65535).contains(number.intValue) else {
            completionHandler(tunnelError("Invalid capture port"))
            return
        }
        let port = number.intValue
        lifecycleLock.lock()
        if isStarting || isRunning {
            lifecycleLock.unlock()
            completionHandler(tunnelError("Tunnel is already starting or running"))
            return
        }
        generation &+= 1
        let startGeneration = generation
        isStarting = true
        isStopping = false
        lifecycleLock.unlock()
        let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
        settings.mtu = 1500
        let ipv4 = NEIPv4Settings(addresses: ["10.111.0.2"], subnetMasks: ["255.255.255.255"])
        ipv4.includedRoutes = [NEIPv4Route.default()]
        ipv4.excludedRoutes = [NEIPv4Route(destinationAddress: "127.0.0.0", subnetMask: "255.0.0.0")]
        settings.ipv4Settings = ipv4
        let ipv6 = NEIPv6Settings(addresses: ["fd00:198:18::2"], networkPrefixLengths: [128])
        ipv6.includedRoutes = [NEIPv6Route.default()]
        settings.ipv6Settings = ipv6
        let dns = NEDNSSettings(servers: ["10.111.0.1"])
        dns.matchDomains = [""]
        settings.dnsSettings = dns
        let proxy = NEProxySettings()
        proxy.httpEnabled = true
        proxy.httpServer = NEProxyServer(address: "127.0.0.1", port: port)
        proxy.httpsEnabled = true
        proxy.httpsServer = NEProxyServer(address: "127.0.0.1", port: port)
        proxy.matchDomains = [""]
        settings.proxySettings = proxy

        // The proxy must be listening before the system advertises it to apps.
        // The packet bridge remains idle until readPackets starts below.
        lifecycleLock.lock()
        if isStopping || generation != startGeneration {
            lifecycleLock.unlock()
            completionHandler(tunnelError("Tunnel stopped while starting"))
            return
        }
        do {
            let dataDirectory = try makeDataDirectory()
            let pointer = dataDirectory.withCString { path in
                hc_ios_start(path, UInt16(port), Self.outputPacket, Unmanaged.passUnretained(self).toOpaque())
            }
            let response = consumeResponse(pointer)
            lifecycleLock.unlock()
            guard response["ok"] as? Bool == true else {
                finishStartup(startGeneration)
                completionHandler(tunnelError(response["error"] as? String ?? "Capture engine failed to start"))
                return
            }
            lifecycleLock.lock()
            let stopped = isStopping || generation != startGeneration
            lifecycleLock.unlock()
            if stopped {
                completionHandler(tunnelError("Tunnel stopped while starting"))
                return
            }
        } catch {
            lifecycleLock.unlock()
            finishStartup(startGeneration)
            completionHandler(error)
            return
        }

        setTunnelNetworkSettings(settings) { error in
            if let error {
                self.lifecycleLock.lock()
                let ownsEngine = self.generation == startGeneration && !self.isStopping
                self.lifecycleLock.unlock()
                if ownsEngine { _ = self.consumeResponse(hc_ios_stop()) }
                self.finishStartup(startGeneration)
                completionHandler(error)
                return
            }
            self.lifecycleLock.lock()
            let stopped = self.isStopping || self.generation != startGeneration
            if !stopped {
                self.isStarting = false
                self.isRunning = true
            }
            self.lifecycleLock.unlock()
            if stopped {
                completionHandler(self.tunnelError("Tunnel stopped while starting"))
                return
            }
            self.readPackets()
            self.startHealthMonitoring(startGeneration)
            completionHandler(nil)
        }
    }

    override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
        lifecycleLock.lock()
        generation &+= 1
        isStopping = true
        isStarting = false
        isRunning = false
        let timer = healthTimer
        healthTimer = nil
        lifecycleLock.unlock()
        timer?.cancel()
        _ = consumeResponse(hc_ios_stop())
        completionHandler()
    }

    private func startHealthMonitoring(_ token: UInt64) {
        let timer = DispatchSource.makeTimerSource(queue: healthQueue)
        timer.schedule(deadline: .now() + .seconds(2), repeating: .seconds(2))
        timer.setEventHandler { [weak self] in self?.checkHealth(token) }
        timer.resume()
        lifecycleLock.lock()
        if isRunning && !isStopping && generation == token {
            healthTimer = timer
            lifecycleLock.unlock()
        } else {
            lifecycleLock.unlock()
            timer.cancel()
        }
    }

    private func checkHealth(_ token: UInt64) {
        lifecycleLock.lock()
        let active = isRunning && !isStopping && generation == token
        lifecycleLock.unlock()
        guard active else { return }
        let response = "{\"method\":\"status\"}".withCString { request in
            consumeResponse(hc_ios_command(request))
        }
        guard response["ok"] as? Bool != true else { return }
        lifecycleLock.lock()
        let stillActive = isRunning && !isStopping && generation == token
        let timer = stillActive ? healthTimer : nil
        if stillActive { healthTimer = nil }
        lifecycleLock.unlock()
        guard stillActive else { return }
        timer?.cancel()
        cancelTunnelWithError(tunnelError(response["error"] as? String ?? "Packet forwarding stopped"))
    }

    override func handleAppMessage(_ messageData: Data, completionHandler: ((Data?) -> Void)?) {
        guard messageData.count <= 128 * 1024 else {
            completionHandler?(try? JSONSerialization.data(withJSONObject:
                ["ok": false, "error": "请求内容超过 128 KiB，请缩小后重试"]))
            return
        }
        guard let request = String(data: messageData, encoding: .utf8) else {
            completionHandler?(try? JSONSerialization.data(withJSONObject:
                ["ok": false, "error": "请求命令不是有效的 UTF-8 文本"]))
            return
        }
        let response = request.withCString { input in consumeResponse(hc_ios_command(input)) }
        completionHandler?(try? JSONSerialization.data(withJSONObject: response))
    }

    private func readPackets() {
        guard running else { return }
        packetFlow.readPackets { [weak self] packets, _ in
            guard let self, self.running else { return }
            for packet in packets {
                packet.withUnsafeBytes { bytes in
                    guard let base = bytes.bindMemory(to: UInt8.self).baseAddress else { return }
                    _ = hc_ios_input_packet(base, bytes.count)
                }
            }
            self.readPackets()
        }
    }

    private static let outputPacket: @convention(c) (UnsafePointer<UInt8>?, Int, UnsafeMutableRawPointer?) -> Void = { bytes, length, context in
        guard let bytes, let context, length > 0 else { return }
        let provider = Unmanaged<PacketTunnelProvider>.fromOpaque(context).takeUnretainedValue()
        let family: Int32
        switch bytes[0] >> 4 {
        case 4: family = AF_INET
        case 6: family = AF_INET6
        default: return
        }
        let packet = Data(bytes: bytes, count: length)
        _ = provider.packetFlow.writePackets([packet], withProtocols: [NSNumber(value: family)])
    }

    private func makeDataDirectory() throws -> String {
        let directory = try FileManager.default.url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("HTTP Capture", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.path
    }

    private func finishStartup(_ token: UInt64) {
        lifecycleLock.lock()
        if generation == token { isStarting = false }
        lifecycleLock.unlock()
    }

    private func consumeResponse(_ pointer: UnsafeMutablePointer<CChar>?) -> [String: Any] {
        guard let pointer else { return ["ok": false, "error": "Native capture returned no result"] }
        defer { hc_ios_free_string(pointer) }
        let data = Data(String(cString: pointer).utf8)
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
            ?? ["ok": false, "error": "Native capture returned invalid JSON"]
    }

    private func tunnelError(_ message: String) -> NSError {
        NSError(domain: "dev.httpcapture.mobile.ios", code: 1, userInfo: [NSLocalizedDescriptionKey: message])
    }
}
