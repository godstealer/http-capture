#!/usr/bin/env python3
"""Run an isolated emulator against the real, authenticated PC LAN gateway.

No credential is passed in adb/am arguments or emitted to CI logs. Camera QR
scanning, real devices, early half-close and PC system-DNS resolution are not
claimed by this test. Production virtual DNS is verified separately.
"""
from __future__ import annotations
import argparse
import base64
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import tempfile
import time
from urllib.parse import parse_qsl, urlsplit

PACKAGE = "dev.httpcapture.mobile"
RUNNER = PACKAGE + ".test/" + PACKAGE + ".CaptureSmokeInstrumentation"
PRIVATE_NETWORKS = tuple(ipaddress.ip_network(n) for n in ("10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"))
MAX_CONFIG = 16 * 1024
ANDROID_PROGRESS = frozenset({
    "config-loaded", "pin-rejected", "replay-rejected",
    "vpn-consent-granted", "vpn-start-requested", "vpn-ready", "traffic-ok", "revoked-ok",
})
ANDROID_FAILED_STAGES = frozenset({
    "config", "pin-check", "pairing", "vpn-consent", "vpn-start", "probe", "revocation",
    "probe-config", "probe-uid-vpn", "probe-http", "probe-udp", "probe-dns-wire", "probe-virtual-dns",
})
HTTP_PROBE_DIAGNOSTICS = (
    ("httpFailureStep", frozenset({
        "connect", "request-one", "response-one-head", "response-one-body",
        "request-two", "response-two-head", "socket-close",
    })),
    ("probeErrorType", frozenset({
        "SocketTimeoutException", "ConnectException", "SocketException", "IllegalStateException",
        "IllegalArgumentException", "EOFException", "SecurityException", "InterruptedException",
        "IOException", "ProbeFailure",
    })),
)
TRAFFIC_FAILURE_CODES = frozenset({
    "stale-android-phase", "invalid-android-phase", "oversized-android-phase",
    "android-reported-failure", "android-ended-before-traffic", "pc-fixture-ended-before-traffic",
    "android-vpn-traffic-timeout",
})

class SmokeError(RuntimeError):
    """Only fixed, credential-free error codes may be constructed here."""


def private_host(value: str) -> str:
    try:
        address = ipaddress.IPv4Address(value)
    except (ValueError, TypeError):
        raise SmokeError("invalid-private-host") from None
    if not any(address in network for network in PRIVATE_NETWORKS):
        raise SmokeError("non-private-host")
    if address in ipaddress.ip_network("10.0.2.0/24") or address in ipaddress.ip_network("10.111.0.0/24"):
        raise SmokeError("emulator-or-tun-host")
    return str(address)


def select_route(routes: list[dict]) -> str:
    for route in routes:
        device = route.get("dev", "")
        if not isinstance(device, str) or re.match(r"^(lo$|docker|br-|veth|tun|tap|wg|utun)", device):
            continue
        source = route.get("prefsrc", route.get("src"))
        try:
            return private_host(source)
        except SmokeError:
            continue
    raise SmokeError("no-private-default-route")


def default_host() -> str:
    # Route lookup does not send a packet to the documentation-only destination.
    result = subprocess.run(["ip", "-j", "-4", "route", "get", "192.0.2.1"],
                            capture_output=True, timeout=10, check=False)
    if result.returncode:
        raise SmokeError("route-lookup-failed")
    try:
        return select_route(json.loads(result.stdout))
    except (ValueError, TypeError):
        raise SmokeError("invalid-route-report") from None


def read_json(path: Path) -> dict:
    data = path.read_bytes()
    if len(data) > MAX_CONFIG:
        raise SmokeError("oversized-private-file")
    try:
        value = json.loads(data)
    except ValueError:
        raise SmokeError("invalid-private-json") from None
    if not isinstance(value, dict):
        raise SmokeError("invalid-private-object")
    return value


def validate_ready(config: dict, host: str, now: int | None = None) -> str:
    if config.get("schemaVersion") != 1 or config.get("host") != private_host(host):
        raise SmokeError("fixture-schema-or-host-mismatch")
    run_id = config.get("runId", "")
    if not isinstance(run_id, str) or not re.fullmatch(r"[a-f0-9]{32}", run_id):
        raise SmokeError("invalid-fixture-run-id")
    ports = [config.get(k) for k in ("gatewayPort", "httpPort", "udpPort", "dnsWirePort")]
    if any(type(p) is not int or not 1024 <= p <= 65535 for p in ports) or len(set(ports)) != len(ports):
        raise SmokeError("invalid-fixture-ports")
    try:
        uri = urlsplit(config["pairingUri"])
        pairs = parse_qsl(uri.query, strict_parsing=True, keep_blank_values=True)
        fields = dict(pairs)
        if uri.scheme != "httpcapture" or uri.netloc != "pair" or uri.path or uri.fragment:
            raise ValueError()
        if len(pairs) != 6 or set(fields) != {"v", "host", "port", "token", "fp", "exp"}:
            raise ValueError()
        if fields["v"] != "1" or fields["host"] != host or int(fields["port"]) != config["gatewayPort"]:
            raise ValueError()
        if not re.fullmatch(r"[A-Za-z0-9_-]{43}", fields["token"]) or not re.fullmatch(r"[a-fA-F0-9]{64}", fields["fp"]):
            raise ValueError()
        expires = int(fields["exp"])
        current = int(time.time()) if now is None else now
        if not current < expires <= current + 300:
            raise ValueError()
        for key in ("expectedRequestBodyBase64", "expectedResponseBodyBase64"):
            body = base64.b64decode(config[key], validate=True)
            if not 1 <= len(body) <= 4096:
                raise ValueError()
    except (KeyError, ValueError, TypeError, OverflowError):
        # URI parsers can include their entire input in exceptions.
        raise SmokeError("invalid-or-expired-private-pairing-config") from None
    return run_id


def verified_report(report: dict, run_id: str) -> None:
    expected = {"status": "ok", "runId": run_id, "httpExchanges": 2,
                "udpDatagrams": 2, "dnsWireDatagrams": 1, "udpSourcePorts": 1,
                "bodyCapture": "notRecorded", "deviceNameMatched": True, "originVerified": True}
    if any(type(report.get(k)) is not type(v) or report.get(k) != v for k, v in expected.items()):
        raise SmokeError("pc-verification-contract-failed")


def stopped_report(report: dict, run_id: str) -> None:
    expected = {"status": "ok", "runId": run_id, "paired": False,
                "deviceNameCleared": True, "pendingFlows": 0}
    if any(type(report.get(k)) is not type(v) or report.get(k) != v for k, v in expected.items()):
        raise SmokeError("pc-revocation-contract-failed")


def sanitized_pc_report(report: dict, *, stopped: bool = False) -> dict:
    keys = ("status", "runId", "paired", "deviceNameCleared", "pendingFlows") if stopped else (
        "status", "runId", "httpExchanges", "udpDatagrams", "dnsWireDatagrams", "udpSourcePorts",
        "bodyCapture", "deviceNameMatched", "originVerified")
    return {key: report[key] for key in keys}


def instrumentation_succeeded(output: str, run_id: str) -> None:
    results = {}
    codes = []
    for line in output.splitlines():
        if line.startswith("INSTRUMENTATION_RESULT: "):
            key, separator, value = line[len("INSTRUMENTATION_RESULT: "):].partition("=")
            if separator:
                if key in results:
                    raise SmokeError("duplicate-instrumentation-result")
                results[key] = value.strip()
        elif line.startswith("INSTRUMENTATION_CODE:"):
            codes.append(line.partition(":")[2].strip())
    if codes != ["-1"] or results.get("lanSmokeStatus") != "ok" or results.get("runId") != run_id:
        raise SmokeError("android-instrumentation-did-not-pass")


def http_probe_diagnostics(report: dict) -> dict:
    return {
        key: report[key] for key, allowed in HTTP_PROBE_DIAGNOSTICS
        if isinstance(report.get(key), str) and report[key] in allowed
    }


def observe_android_phase(report: dict, run_id: str, progress: dict) -> str:
    if report.get("runId") != run_id:
        raise SmokeError("stale-android-phase")
    stage = report.get("phase")
    if stage == "failed":
        failed_stage = report.get("failedStage")
        if isinstance(failed_stage, str) and failed_stage in ANDROID_FAILED_STAGES:
            progress["androidFailedStage"] = failed_stage
            if failed_stage == "probe-http":
                progress.update(http_probe_diagnostics(report))
    elif isinstance(stage, str) and stage in ANDROID_PROGRESS:
        progress["androidLastPhase"] = stage
    else:
        raise SmokeError("invalid-android-phase")
    return stage


def failure_summary(progress: dict, error: BaseException) -> dict:
    summary = {"schemaVersion": 1, "status": "failed", "phase": progress["phase"],
               "errorType": type(error).__name__}
    for key, allowed in (("androidLastPhase", ANDROID_PROGRESS), ("androidFailedStage", ANDROID_FAILED_STAGES)):
        value = progress.get(key)
        if isinstance(value, str) and value in allowed:
            summary[key] = value
    if summary.get("androidFailedStage") == "probe-http":
        summary.update(http_probe_diagnostics(progress))
    # Only our exact diagnostic codes are public. Arbitrary exception text is private.
    if isinstance(error, SmokeError) and error.args and isinstance(error.args[0], str):
        code = error.args[0]
        if code in TRAFFIC_FAILURE_CODES:
            summary["failureCode"] = code
    return summary


def wait_file(path: Path, process: subprocess.Popen, seconds: float) -> dict:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if path.is_file():
            try:
                return read_json(path)
            except SmokeError as error:
                if str(error) != "invalid-private-json":
                    raise
                # create_new files can be visible just before sync_all finishes.
                # Retry only incomplete JSON, not a valid but wrong contract.
        if process.poll() is not None:
            raise SmokeError("pc-fixture-exited")
        time.sleep(0.15)
    raise SmokeError("pc-fixture-file-timeout")


def fixture_command(process: subprocess.Popen, command: str) -> None:
    if process.poll() is not None or process.stdin is None:
        raise SmokeError("pc-fixture-not-running")
    process.stdin.write((command + "\n").encode("ascii"))
    process.stdin.flush()


class Adb:
    def __init__(self, binary: str, serial: str | None):
        self.binary = binary
        if serial is None:
            listing = subprocess.run([binary, "devices"], capture_output=True, timeout=15, check=False)
            devices = [line.split()[0] for line in listing.stdout.decode("utf-8", "replace").splitlines()
                       if re.match(r"^emulator-[0-9]+\s+device$", line.strip())]
            if listing.returncode or len(devices) != 1:
                raise SmokeError("one-test-emulator-is-required")
            serial = devices[0]
        if not re.fullmatch(r"emulator-[0-9]+", serial):
            raise SmokeError("physical-device-is-not-allowed")
        self.command = [binary, "-s", serial]
        if self.run(["shell", "getprop", "ro.boot.qemu"]).stdout.strip() != b"1":
            raise SmokeError("target-is-not-an-emulator")
        if self.run(["shell", "am", "get-current-user"]).stdout.strip() != b"0":
            raise SmokeError("test-emulator-user-must-be-zero")
        # Also fail before creating a fresh five-minute credential if app/runner
        # installation or the target app's private directory is not ready.
        private_dir = self.run(["shell", "run-as", PACKAGE, "pwd"]).stdout.decode().strip()
        if not re.fullmatch(r"/data/(?:user/0|data)/dev\.httpcapture\.mobile", private_dir):
            raise SmokeError("invalid-app-private-directory")
        self.private_dir = private_dir

    def run(self, arguments: list[str], *, data: bytes | None = None,
            timeout: float = 15, check: bool = True) -> subprocess.CompletedProcess:
        result = subprocess.run(self.command + arguments, input=data, capture_output=True,
                                timeout=timeout, check=False)
        if check and result.returncode:
            raise SmokeError("adb-operation-failed")
        return result

    def write_private(self, name: str, data: bytes) -> None:
        if not re.fullmatch(r"httpcapture-lan-[a-f0-9]{32}\.json", name):
            raise SmokeError("invalid-private-file-name")
        command = "run-as " + PACKAGE + " sh -c " + shlex.quote("umask 077; set -C; cat > files/" + name)
        self.run(["shell", "-T", command], data=data)

    def phase(self, name: str) -> dict | None:
        result = self.run(["shell", "run-as", PACKAGE, "cat", "files/" + name], check=False, timeout=5)
        if result.returncode:
            return None
        if len(result.stdout) > MAX_CONFIG:
            raise SmokeError("oversized-android-phase")
        try:
            value = json.loads(result.stdout)
            if isinstance(value, dict):
                return value
        except ValueError:
            pass  # An atomic phase file may not have appeared yet.
        return None

    def remove_private(self, names: list[str]) -> None:
        for name in names:
            if not re.fullmatch(r"httpcapture-lan-[a-f0-9]{32}(?:-phase)?\.json", name):
                raise SmokeError("invalid-private-cleanup-name")
        private_paths = ["files/" + name for name in names]
        for name in names:
            if name.endswith("-phase.json"):
                private_paths.extend("files/" + name + suffix for suffix in (".new", ".bak"))
        self.run(["shell", "run-as", PACKAGE, "rm", "-f", *private_paths], check=False)


def run_case(args: argparse.Namespace, progress: dict) -> dict:
    progress["phase"] = "emulator-preflight"
    adb = Adb(args.adb, args.serial)
    fixture_binary = Path(args.fixture).resolve(strict=True)
    if not fixture_binary.is_file():
        raise SmokeError("fixture-binary-is-missing")
    host = private_host(args.host) if args.host else default_host()
    previous_umask = os.umask(0o077)
    try:
        with tempfile.TemporaryDirectory(prefix="http-capture-lan-e2e-") as temporary:
            root = Path(temporary).resolve()
            state = root / "pc"
            state.mkdir(mode=0o700)
            fixture = None
            instrumentation = None
            names = []
            with (root / "pc-private.log").open("xb") as pc_log, (root / "android-private.log").open("xb") as android_log:
                try:
                    progress["phase"] = "pc-fixture-start"
                    fixture = subprocess.Popen([str(fixture_binary), host, str(state)],
                                               stdin=subprocess.PIPE, stdout=pc_log, stderr=subprocess.STDOUT,
                                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                    ready = wait_file(state / "ready.json", fixture, 30)
                    run_id = validate_ready(ready, host)
                    progress["runId"] = run_id
                    name = "httpcapture-lan-" + run_id + ".json"
                    phase_name = "httpcapture-lan-" + run_id + "-phase.json"
                    names = [name, phase_name]
                    progress["phase"] = "android-private-config"
                    adb.write_private(name, json.dumps(ready, separators=(",", ":")).encode("utf-8"))
                    # Only paths are process arguments. All one-use credentials
                    # crossed adb stdin directly into the app's private directory.
                    command = adb.command + ["shell", "am", "instrument", "-w",
                        "-e", "lanConfigPath", adb.private_dir + "/files/" + name,
                        "-e", "lanPhasePath", adb.private_dir + "/files/" + phase_name, RUNNER]
                    instrumentation = subprocess.Popen(command, stdout=android_log, stderr=subprocess.STDOUT,
                                                       creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                    progress["phase"] = "android-vpn-traffic"
                    deadline = time.monotonic() + 130
                    while time.monotonic() < deadline:
                        finished = instrumentation.poll() is not None
                        report = adb.phase(phase_name)
                        if report is not None:
                            stage = observe_android_phase(report, run_id, progress)
                            if stage == "traffic-ok":
                                break
                            if stage == "failed":
                                raise SmokeError("android-reported-failure")
                        if finished:
                            raise SmokeError("android-ended-before-traffic")
                        if fixture.poll() is not None:
                            raise SmokeError("pc-fixture-ended-before-traffic")
                        time.sleep(0.3)
                    else:
                        raise SmokeError("android-vpn-traffic-timeout")
                    progress["phase"] = "pc-flow-verification"
                    fixture_command(fixture, "verify")
                    pc = wait_file(state / "verification.json", fixture, 35)
                    verified_report(pc, run_id)
                    progress["phase"] = "pc-session-revocation"
                    fixture_command(fixture, "disconnect")
                    disconnected = wait_file(state / "disconnected.json", fixture, 5)
                    if disconnected.get("runId") != run_id:
                        raise SmokeError("invalid-pc-disconnect-report")
                    progress["phase"] = "android-revocation-cleanup"
                    if instrumentation.wait(timeout=115) != 0:
                        raise SmokeError("adb-instrumentation-process-failed")
                    android_log.flush()
                    instrumentation_succeeded((root / "android-private.log").read_text(encoding="utf-8", errors="replace"), run_id)
                    progress["phase"] = "pc-revocation-verification"
                    fixture_command(fixture, "verify-stopped")
                    stopped = wait_file(state / "stopped.json", fixture, 35)
                    stopped_report(stopped, run_id)
                    fixture_command(fixture, "quit")
                    if fixture.wait(timeout=10) != 0:
                        raise SmokeError("pc-fixture-shutdown-failed")
                    progress["phase"] = "completed"
                    return {"schemaVersion": 1, "status": "ok", "runId": run_id,
                            "test": "android-emulator-to-production-pc-gateway",
                            "pcVerification": sanitized_pc_report(pc), "pcRevocation": sanitized_pc_report(stopped, stopped=True),
                            "androidInstrumentation": "passed", "systemDnsMode": "virtual",
                            "systemDnsForwardedToPc": False, "dnsWireOverUdp": True,
                            "captureCaInstalled": False,
                            "remaining": ["camera-scan", "physical-device", "network-switch", "early-half-close"]}
                finally:
                    # Revoke before forced teardown. Every process below was
                    # started by this invocation; no user listener is discovered
                    # or killed by port number.
                    if fixture is not None and fixture.poll() is None:
                        try:
                            fixture_command(fixture, "disconnect")
                        except (OSError, SmokeError):
                            pass
                    if instrumentation is not None and instrumentation.poll() is None:
                        try:
                            adb.run(["shell", "am", "force-stop", PACKAGE], timeout=8, check=False)
                        except (OSError, subprocess.TimeoutExpired):
                            pass
                        try:
                            instrumentation.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            instrumentation.terminate()
                            try:
                                instrumentation.wait(timeout=5)
                            except subprocess.TimeoutExpired:
                                instrumentation.kill()
                                instrumentation.wait(timeout=5)
                    if names:
                        try:
                            adb.remove_private(names)
                        except (OSError, subprocess.TimeoutExpired):
                            pass
                    if fixture is not None:
                        if fixture.poll() is None:
                            try:
                                fixture_command(fixture, "quit")
                                fixture.wait(timeout=8)
                            except (OSError, SmokeError, subprocess.TimeoutExpired):
                                fixture.terminate()
                                try:
                                    fixture.wait(timeout=5)
                                except subprocess.TimeoutExpired:
                                    fixture.kill()
                                    fixture.wait(timeout=5)
                        if fixture.stdin is not None:
                            fixture.stdin.close()
    finally:
        os.umask(previous_umask)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixture", required=True, help="Path to the separately built PC fixture executable")
    parser.add_argument("--adb", default="adb")
    parser.add_argument("--serial", help="A test emulator serial; physical devices are rejected")
    parser.add_argument("--host", help="Real RFC1918 host address; otherwise use the Linux default route")
    parser.add_argument("--summary", type=Path, help="Sanitized report; no private config or logs are copied here")
    args = parser.parse_args()
    progress = {"phase": "preflight"}
    try:
        summary = run_case(args, progress)
        code = 0
        print("LAN E2E passed: production pinned pairing, two HTTP exchanges, UDP/DNS-wire, and session revocation")
    except (Exception, KeyboardInterrupt) as error:
        summary = failure_summary(progress, error)
        # Never print adb output, exception messages, arguments, private JSON,
        # logcat, or full Bundles; those can contain the one-use pairing URI.
        print("LAN E2E failed at " + progress["phase"] + " (" + type(error).__name__ + ")")
        code = 1
    if args.summary:
        args.summary.write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    return code


if __name__ == "__main__":
    raise SystemExit(main())
