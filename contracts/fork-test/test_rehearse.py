import importlib.util
from pathlib import Path
import unittest
import subprocess
import sys
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "staging_rehearsal", Path(__file__).with_name("rehearse.py")
)
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)


class LocalOnlyGuards(unittest.TestCase):
    def test_upstream_mutations_fail_before_network(self):
        rpc = staging.RPC("https://ethereum-sepolia-rpc.publicnode.com")
        with patch.object(staging.urllib.request, "urlopen") as network:
            for method in [
                "eth_sendTransaction",
                "eth_sendRawTransaction",
                "anvil_impersonateAccount",
                "anvil_setStorageAt",
            ]:
                with self.subTest(method=method), self.assertRaisesRegex(
                    AssertionError, "upstream write forbidden"
                ):
                    rpc.call(method, [])
            network.assert_not_called()

    def test_write_endpoint_requires_explicit_loopback(self):
        for url in [
            "https://example.com",
            "http://example.com:8545",
            "http://127.0.0.1@example.com:8545",
            "http://localhost:8545",
        ]:
            with self.subTest(url=url), self.assertRaisesRegex(
                AssertionError, "loopback"
            ):
                staging.RPC(url, local=True)

    def test_send_rejects_upstream_even_when_it_reports_anvil(self):
        rpc = staging.RPC("https://example.com")
        with patch.object(rpc, "call", return_value="anvil/v1") as calls:
            with self.assertRaisesRegex(AssertionError, "Anvil"):
                rpc.send(None, "0x")
            calls.assert_not_called()

    def test_send_rejects_non_anvil_local_service(self):
        rpc = staging.RPC("http://127.0.0.1:8545", local=True)
        with patch.object(rpc, "call", return_value="reth/v1") as calls:
            with self.assertRaisesRegex(AssertionError, "Anvil"):
                rpc.send(None, "0x")
            self.assertEqual(calls.call_args_list[0].args, ("web3_clientVersion", []))
            self.assertEqual(calls.call_count, 1)

    def test_deployment_rejects_upstream_before_invoking_script(self):
        rpc = staging.RPC("https://example.com")
        with patch.object(staging.subprocess, "run") as deployment:
            with self.assertRaisesRegex(AssertionError, "loopback Anvil"):
                staging.deploy_with_repo_script(
                    rpc, "base", "base", "0x01", Path("/unused")
                )
            deployment.assert_not_called()

    def test_snapshot_hash_mismatch_stops_before_anvil(self):
        from unittest.mock import Mock

        rpc = Mock()
        rpc.call.side_effect = [hex(84532), {"hash": "0xchanged"}]
        manifest = {
            "targets": [
                {
                    "environment": "rehearsal",
                    "contract": "base",
                    "chain_id": 84532,
                    "baseline_hash_block": 123,
                    "baseline_hash_block_hash": "0xexpected",
                }
            ]
        }
        with patch.object(staging, "RPC", return_value=rpc), patch.object(
            staging.subprocess, "Popen"
        ) as anvil:
            with self.assertRaisesRegex(AssertionError, "snapshot hash changed"):
                staging.run_one("base", Path("/unused"), manifest)
            anvil.assert_not_called()

    def test_python_optimization_cannot_disable_guards(self):
        for script in ["prepare.py", "rehearse.py"]:
            completed = subprocess.run(
                [sys.executable, "-O", str(Path(__file__).with_name(script)), "--help"],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("without Python optimization", completed.stderr)


if __name__ == "__main__":
    unittest.main()
