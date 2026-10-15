import subprocess
import unittest
from unittest.mock import Mock
from types import SimpleNamespace
from run_live_session import CoreClient, session_duration_active, session_ready, acquire_session, v2pop_command

class LiveRunnerTests(unittest.TestCase):
    def test_resume_claims_exact_seat_without_create_or_stop(self):
        core = Mock()
        session = {"sessionId": "dota-seat", "status": 2, "resumePending": True}
        core.call.side_effect = [("1", {"sessions": [session]}), ("2", {"session": session})]
        self.assertEqual(acquire_session(core, "dota-seat", "9029111", "Dota 2", {}), session)
        self.assertEqual([c.args for c in core.call.call_args_list],
                         [("session.remote.list",), ("session.claim", {"sessionId": "dota-seat"})])
        core.send.assert_not_called()

    def test_missing_resume_seat_never_falls_back_to_allocation(self):
        core = Mock()
        core.call.return_value = ("1", {"sessions": [{"sessionId": "other-seat"}]})
        with self.assertRaisesRegex(RuntimeError, "refusing a new allocation"):
            acquire_session(core, "dota-seat", "9029111", "Dota 2", {})
        core.call.assert_called_once_with("session.remote.list")

    def test_discovery_failure_never_claims_or_creates(self):
        core = Mock()
        core.call.side_effect = RuntimeError("session_discovery_failed")
        with self.assertRaisesRegex(RuntimeError, "session_discovery_failed"):
            acquire_session(core, "dota-seat", "9029111", "Dota 2", {})
        core.call.assert_called_once_with("session.remote.list")

    def test_claim_readiness_requires_fresh_poll(self):
        self.assertFalse(session_ready({"status": 2, "resumePending": True}))
        self.assertFalse(session_ready({"status": 1}))
        self.assertTrue(session_ready({"status": 2}))
        self.assertTrue(session_ready({"status": 3, "resumePending": False}))

    def _pop_args(self, **extra):
        base = dict(v2pop_bin="v2pop", relay_key="k", gfn_key="g", gfn_salt="s", port=29878, codec="av1",
                    route=None, trunk_peer=None, port_range=None)
        base.update(extra)
        return SimpleNamespace(**base)

    def test_v2pop_without_route_is_unchanged(self):
        cmd = v2pop_command(self._pop_args(), "a", "ctx.json")
        self.assertNotIn("--route", cmd)
        self.assertEqual(cmd[-4:], ["--context", "ctx.json", "--sim", "0"])

    def test_v2pop_route_passes_trunk_peer_and_port_range(self):
        cmd = v2pop_command(self._pop_args(route="r.json", trunk_peer="45.115.216.138", port_range="29870-29899"),
                            "a", "ctx.json")
        self.assertEqual(cmd[-6:], ["--route", "r.json", "--trunk-peer", "45.115.216.138",
                                    "--port-range", "29870-29899"])

    def test_new_allocation_is_acknowledged(self):
        core = Mock()
        session = {"sessionId": "fresh-seat", "status": 1}
        core.call.return_value = ("7", {"session": session})
        self.assertEqual(acquire_session(core, None, "9029111", "Dota 2", {"generation": 1}), session)
        core.call.assert_called_once_with("session.create", {
            "appId": "9029111", "variantId": "9029111", "scope": {"generation": 1}, "title": "Dota 2"
        })
        core.send.assert_called_once_with({"type": "ack", "id": "7"})

    def test_stream_has_no_default_duration_limit(self):
        self.assertTrue(session_duration_active(0, 86400))
        self.assertTrue(session_duration_active(30, 29))
        self.assertFalse(session_duration_active(30, 30))

    def test_core_is_reaped_when_stdin_close_does_not_stop_it(self):
        core = CoreClient.__new__(CoreClient)
        core.proc = Mock()
        core.proc.wait.side_effect = [subprocess.TimeoutExpired("core", 5),
                                      subprocess.TimeoutExpired("core", 5), 0]
        core.close()
        core.proc.stdin.close.assert_called_once()
        core.proc.terminate.assert_called_once()
        core.proc.kill.assert_called_once()
        self.assertEqual(core.proc.wait.call_count, 3)

    def test_core_normal_close_does_not_send_signals(self):
        core = CoreClient.__new__(CoreClient)
        core.proc = Mock()
        core.close()
        core.proc.terminate.assert_not_called()
        core.proc.kill.assert_not_called()

if __name__ == "__main__":
    unittest.main()
