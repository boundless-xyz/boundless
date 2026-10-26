import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("staging_rehearsal", Path(__file__).with_name("rehearse-staging.py"))
staging = importlib.util.module_from_spec(spec)
spec.loader.exec_module(staging)


class LocalOnlyGuards(unittest.TestCase):
    def test_upstream_mutations_fail_before_network(self):
        rpc = staging.RPC("https://ethereum-sepolia-rpc.publicnode.com")
        with patch.object(staging.urllib.request, "urlopen") as network:
            for method in ["eth_sendTransaction", "eth_sendRawTransaction", "anvil_impersonateAccount", "anvil_setStorageAt"]:
                with self.subTest(method=method), self.assertRaisesRegex(AssertionError, "upstream write forbidden"):
                    rpc.call(method, [])
            network.assert_not_called()

    def test_write_endpoint_requires_explicit_loopback(self):
        for url in ["https://example.com", "http://example.com:8545", "http://127.0.0.1@example.com:8545", "http://localhost:8545"]:
            with self.subTest(url=url), self.assertRaisesRegex(AssertionError, "loopback"):
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


if __name__ == "__main__":
    unittest.main()
