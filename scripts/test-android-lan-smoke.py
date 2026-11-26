#!/usr/bin/env python3
"""Credential, source-identity and false-success boundaries; no adb/network IO."""
import base64
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True

spec = importlib.util.spec_from_file_location("android_lan_smoke", Path(__file__).with_name("smoke-android-lan.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
RUN_ID = "c1" * 16
SECRET = "PRIVATE_PAIRING_SECRET_MUST_NOT_APPEAR"


def configuration():
    return {"schemaVersion": 1, "runId": RUN_ID, "host": "192.168.1.10",
            "gatewayPort": 12345, "httpPort": 12346, "udpPort": 12347, "dnsWirePort": 12348,
            "pairingUri": "httpcapture://pair?v=1&host=192.168.1.10&port=12345&token=" + "A" * 42 + "E" + "&fp=" + "ab" * 32 + "&exp=1120",
            "expectedRequestBodyBase64": base64.b64encode(b"request").decode(),
            "expectedResponseBodyBase64": base64.b64encode(b"response").decode()}


def verified():
    return {"status": "ok", "runId": RUN_ID, "httpExchanges": 2, "udpDatagrams": 2,
            "dnsWireDatagrams": 1, "udpSourcePorts": 1, "bodyCapture": "notRecorded",
            "deviceNameMatched": True, "originVerified": True}


class LanSmokeBoundaries(unittest.TestCase):
    def test_real_private_default_route_excludes_emulator_and_tun_aliases(self):
        self.assertEqual(smoke.select_route([
            {"dev": "docker0", "prefsrc": "172.17.0.1"},
            {"dev": "tun0", "prefsrc": "10.20.0.1"},
            {"dev": "eth0", "prefsrc": "192.168.1.10"}]), "192.168.1.10")
        for host in ["127.0.0.1", "10.0.2.2", "10.111.0.1", "8.8.8.8", "192.0.2.1", "172.32.0.1"]:
            with self.subTest(host=host), self.assertRaises(smoke.SmokeError):
                smoke.private_host(host)

    def test_current_same_host_pairing_configuration_is_required(self):
        self.assertEqual(smoke.validate_ready(configuration(), "192.168.1.10", 1000), RUN_ID)
        for replace in ["host=192.168.1.11", "port=12349", "exp=999", "exp=1400", "v=2"]:
            config = configuration()
            field = replace.partition("=")[0]
            original = next(part for part in config["pairingUri"].split("?")[1].split("&") if part.startswith(field + "="))
            config["pairingUri"] = config["pairingUri"].replace(original, replace)
            with self.subTest(field=field), self.assertRaises(smoke.SmokeError):
                smoke.validate_ready(config, "192.168.1.10", 1000)

    def test_duplicate_uri_fields_and_false_ports_are_rejected(self):
        for mutate in [lambda c: c.update(pairingUri=c["pairingUri"] + "&port=12345"),
                       lambda c: c.update(gatewayPort=True),
                       lambda c: c.update(udpPort=c["dnsWirePort"]),
                       lambda c: c.update(runId="../wrong")]:
            config = configuration()
            mutate(config)
            with self.assertRaises(smoke.SmokeError):
                smoke.validate_ready(config, "192.168.1.10", 1000)

    def test_pairing_errors_do_not_echo_private_uri(self):
        config = configuration()
        config["pairingUri"] = "httpcapture://[" + SECRET
        with self.assertRaises(smoke.SmokeError) as caught:
            smoke.validate_ready(config, "192.168.1.10", 1000)
        self.assertNotIn(SECRET, str(caught.exception))

    def test_physical_devices_are_never_selected(self):
        with mock.patch.object(smoke.subprocess, "run") as run:
            with self.assertRaises(smoke.SmokeError):
                smoke.Adb("adb", "physical-device")
            run.assert_not_called()

    def test_credential_is_only_adb_stdin_not_process_arguments(self):
        adb = object.__new__(smoke.Adb)
        adb.run = mock.Mock()
        name = "httpcapture-lan-" + RUN_ID + ".json"
        payload = json.dumps({"pairingUri": SECRET}).encode()
        adb.write_private(name, payload)
        arguments, keywords = adb.run.call_args
        self.assertNotIn(SECRET, str(arguments))
        self.assertEqual(keywords["data"], payload)
        self.assertIn("umask 077", str(arguments))
        self.assertIn("set -C", str(arguments))

    def test_pc_verification_requires_real_typed_counts_and_body_state(self):
        smoke.verified_report(verified(), RUN_ID)
        for key, wrong in [("httpExchanges", True), ("udpDatagrams", 0), ("dnsWireDatagrams", 0),
                           ("udpSourcePorts", 2), ("bodyCapture", "recorded"),
                           ("deviceNameMatched", False), ("originVerified", False), ("runId", "other")]:
            report = verified()
            report[key] = wrong
            with self.subTest(key=key), self.assertRaises(smoke.SmokeError):
                smoke.verified_report(report, RUN_ID)

    def test_stopped_report_cannot_claim_success_while_still_paired(self):
        report = {"status": "ok", "runId": RUN_ID, "paired": False, "deviceNameCleared": True, "pendingFlows": 0}
        smoke.stopped_report(report, RUN_ID)
        for key, value in [("paired", True), ("deviceNameCleared", False), ("pendingFlows", 1)]:
            wrong = dict(report, **{key: value})
            with self.assertRaises(smoke.SmokeError):
                smoke.stopped_report(wrong, RUN_ID)

    def test_arbitrary_fixture_extras_never_enter_public_report(self):
        report = verified()
        report.update(pairingUri=SECRET, token=SECRET, fullFlow={"authorization": SECRET})
        public = smoke.sanitized_pc_report(report)
        self.assertNotIn(SECRET, json.dumps(public))
        self.assertEqual(public, verified())

    def test_instrumentation_requires_matching_identity_and_explicit_success(self):
        success = "INSTRUMENTATION_RESULT: lanSmokeStatus=ok\nINSTRUMENTATION_RESULT: runId=" + RUN_ID + "\nINSTRUMENTATION_CODE: -1\n"
        smoke.instrumentation_succeeded(success, RUN_ID)
        for wrong in [success.replace("=ok", "=failed"), success.replace(RUN_ID, "other"),
                      success.replace(": -1", ": 0"), success + "INSTRUMENTATION_CODE: -1\n",
                      success + "INSTRUMENTATION_RESULT: lanSmokeStatus=ok\n"]:
            with self.assertRaises(smoke.SmokeError):
                smoke.instrumentation_succeeded(wrong, RUN_ID)

    def test_partially_written_private_json_is_retried_before_contract_validation(self):
        path = mock.Mock()
        path.is_file.return_value = True
        path.read_bytes.side_effect = [b"{", b'{"status":"ok"}']
        process = mock.Mock()
        process.poll.return_value = None
        with mock.patch.object(smoke.time, "sleep"):
            self.assertEqual(smoke.wait_file(path, process, 1), {"status": "ok"})

    def test_known_android_phases_report_only_the_last_milestone_and_failure_stage(self):
        progress = {"phase": "android-vpn-traffic", "runId": RUN_ID}
        for milestone in ("config-loaded", "pin-rejected", "replay-rejected", "vpn-ready", "traffic-ok"):
            phase = {"runId": RUN_ID, "phase": milestone, "pairingUri": SECRET}
            self.assertEqual(smoke.observe_android_phase(phase, RUN_ID, progress), milestone)
            self.assertEqual(progress["androidLastPhase"], milestone)

        failed = {"runId": RUN_ID, "phase": "failed", "failedStage": "probe-http",
                  "errorMessage": SECRET}
        self.assertEqual(smoke.observe_android_phase(failed, RUN_ID, progress), "failed")
        report = smoke.failure_summary(progress, smoke.SmokeError("android-reported-failure"))
        self.assertEqual(report, {
            "schemaVersion": 1, "status": "failed", "phase": "android-vpn-traffic",
            "errorType": "SmokeError", "failureCode": "android-reported-failure",
            "androidLastPhase": "traffic-ok", "androidFailedStage": "probe-http",
        })

    def test_stale_and_unknown_android_phase_snapshots_do_not_change_progress(self):
        progress = {"phase": "android-vpn-traffic", "androidLastPhase": "vpn-ready"}
        for payload, reason in [
            ({"runId": "other", "phase": "traffic-ok"}, "stale-android-phase"),
            ({"runId": RUN_ID, "phase": SECRET}, "invalid-android-phase"),
            ({"runId": RUN_ID, "phase": [SECRET]}, "invalid-android-phase"),
        ]:
            with self.subTest(reason=reason), self.assertRaisesRegex(smoke.SmokeError, "^" + reason + "$"):
                smoke.observe_android_phase(payload, RUN_ID, progress)
            self.assertEqual(progress, {"phase": "android-vpn-traffic", "androidLastPhase": "vpn-ready"})

    def test_failure_summary_omits_untrusted_optional_diagnostics(self):
        progress = {"phase": "android-vpn-traffic", "runId": RUN_ID}
        failed = {"runId": RUN_ID, "phase": "failed", "failedStage": SECRET}
        self.assertEqual(smoke.observe_android_phase(failed, RUN_ID, progress), "failed")
        for error in (smoke.SmokeError(SECRET), ValueError(SECRET)):
            summary = smoke.failure_summary(progress, error)
            self.assertEqual(summary, {
                "schemaVersion": 1, "status": "failed", "phase": "android-vpn-traffic",
                "errorType": type(error).__name__,
            })
            self.assertNotIn(SECRET, json.dumps(summary))
            self.assertNotIn(RUN_ID, json.dumps(summary))

    def test_failure_output_and_summary_do_not_publish_exception_messages(self):
        with tempfile.TemporaryDirectory() as directory:
            summary = Path(directory) / "public.json"
            output = io.StringIO()
            with mock.patch.object(sys, "argv", ["smoke", "--fixture", "unused", "--summary", str(summary)]), \
                 mock.patch.object(smoke, "run_case", side_effect=ValueError(SECRET)), contextlib.redirect_stdout(output):
                self.assertEqual(smoke.main(), 1)
            self.assertNotIn(SECRET, output.getvalue() + summary.read_text())
            self.assertEqual(json.loads(summary.read_text())["status"], "failed")


if __name__ == "__main__":
    unittest.main()
