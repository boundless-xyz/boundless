"""Transport-level preflight guards; real encoding and EVM execution are checked on Anvil."""
import copy
import unittest
from unittest.mock import patch
import simulate

class PreflightTests(unittest.TestCase):
    def setUp(self):
        self.tx = {"chainId": 1, "blockNumber": 100, "safe": "0x" + "a" * 40,
                   "to": "0x" + "b" * 40, "value": 0, "operation": 0, "data": "0x1234",
                   "safeTxGas": 0, "baseGas": 0, "gasPrice": 0,
                   "gasToken": simulate.ZERO, "refundReceiver": simulate.ZERO,
                   "nonce": 2, "threshold": 3, "safeTxHash": "0x" + "c" * 64,
                   "owners": ["0x" + format(n, "040x") for n in range(1, 7)]}
        self.calls = []
        self.chain, self.nonce, self.threshold = 1, 2, 3
        self.hash, self.success = self.tx["safeTxHash"], True
        self.encoder = patch.object(simulate, "encode", side_effect=lambda signature, *args: signature)
        self.transport = patch.object(simulate, "rpc", side_effect=self.rpc)
        self.encoder.start()
        self.transport.start()
        self.addCleanup(self.encoder.stop)
        self.addCleanup(self.transport.stop)

    def rpc(self, url, method, params):
        self.calls.append((method, copy.deepcopy(params)))
        if method == "eth_chainId": return hex(self.chain)
        if method == "eth_getStorageAt": return hex(self.threshold)
        data = params[0]["data"]
        if data == "nonce()": return hex(self.nonce)
        if data.startswith("getTransactionHash("): return self.hash
        return hex(int(self.success))

    def test_rejects_missing_or_ambiguous_printer_output(self):
        with self.assertRaises(ValueError): simulate.extract("no transaction")
        with self.assertRaises(ValueError): simulate.extract("SUNSET_TX_JSON={}\nSUNSET_TX_JSON={}")

    def test_uses_only_read_rpc_methods_and_labels_override(self):
        result = simulate.simulate(self.tx, "mock://rpc")
        self.assertEqual(result["preflight"], "success")
        self.assertEqual(result["simulationThreshold"], 1)
        self.assertEqual(result["actualThreshold"], 3)
        self.assertTrue(all(method in {"eth_chainId", "eth_call", "eth_getStorageAt"} for method, _ in self.calls))
        _, params = self.calls[-1]
        self.assertEqual(params[1], hex(self.tx["blockNumber"]))
        self.assertEqual(int(next(iter(params[2][self.tx["safe"]]["stateDiff"].values())), 16), 1)

    def test_rejects_wrong_chain(self):
        self.chain = 8453
        with self.assertRaisesRegex(ValueError, "chain"): simulate.simulate(self.tx, "mock://rpc")

    def test_rejects_changed_nonce(self):
        self.nonce = 3
        with self.assertRaisesRegex(ValueError, "nonce"): simulate.simulate(self.tx, "mock://rpc")

    def test_rejects_changed_payload_hash(self):
        self.hash = "0x" + "d" * 64
        with self.assertRaisesRegex(ValueError, "hash"): simulate.simulate(self.tx, "mock://rpc")

    def test_rejects_unexpected_threshold_layout(self):
        self.threshold = 4
        with self.assertRaisesRegex(ValueError, "threshold"): simulate.simulate(self.tx, "mock://rpc")

    def test_rejects_failed_safe_execution(self):
        self.success = False
        with self.assertRaisesRegex(ValueError, "return true"): simulate.simulate(self.tx, "mock://rpc")

    def test_rejects_nonzero_value(self):
        self.tx["value"] = 1
        with self.assertRaisesRegex(ValueError, "value"): simulate.simulate(self.tx, "mock://rpc")

if __name__ == "__main__":
    unittest.main()
