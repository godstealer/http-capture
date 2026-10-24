"""Local UDP echo origin for the Android VPN emulator smoke test."""

import socket


with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as server:
    server.bind(("127.0.0.1", 18082))
    print("UDP echo ready", flush=True)
    while True:
        payload, peer = server.recvfrom(64)
        if payload == b"vpn-udp-smoke-01":
            print("VPN UDP marker received", flush=True)
            server.sendto(payload, peer)
