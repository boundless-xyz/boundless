import copy
import unittest
from unittest.mock import patch
import bundle_ethereum as bundle
import simulate

class BundleTests(unittest.TestCase):
    def setUp(self):
        self.impls = ["0x" + format(i, "040x") for i in [101, 102, 103]]
        self.calls = [(proxy, simulate.encode("upgradeToAndCall(address,bytes)", impl, "0x")) for proxy, impl in zip([bundle.ACCOUNTING, bundle.MINT, bundle.ZKC], self.impls)]
        def pack(calls):
            return "0x" + b"".join(bytes([0]) + bytes.fromhex(to[2:]) + bytes(32) + ((len(data)-2)//2).to_bytes(32, "big") + bytes.fromhex(data[2:]) for to, data in calls).hex()
        self.pack = pack
        self.povw = {"chainId": 1, "safe": bundle.SAFE, "blockNumber": 100, "nonce": 10, "owners": [], "threshold": 3,
                     "value": 0, "safeTxGas": 0, "baseGas": 0, "gasPrice": 0, "gasToken": simulate.ZERO, "refundReceiver": simulate.ZERO,
                     "to": bundle.MULTISEND, "operation": 1, "data": simulate.encode("multiSend(bytes)", pack(self.calls[:2])), "safeTxHash": "0x"+"a"*64}
        self.zkc = copy.deepcopy(self.povw)
        self.zkc.update(to=bundle.ZKC, operation=0, data=self.calls[2][1], safeTxHash="0x"+"b"*64)

    def test_exact_order_and_one_safe_hash(self):
        self.povw["description"] = "old two-call description"
        with patch.object(simulate, "simulate", return_value={}), patch.object(simulate, "rpc", return_value="0x"+"c"*64):
            result = bundle.combine(self.povw, self.zkc, "mock://rpc")
        self.assertEqual(bundle.unpack_calls(result["data"]), self.calls)
        self.assertEqual([step["implementation"] for step in result["steps"]], self.impls)
        self.assertEqual(result["nonce"], 10)
        self.assertEqual(result["safeTxHash"], "0x"+"c"*64)
        self.assertNotIn("description", result)

    def test_rejects_different_snapshot(self):
        self.zkc["blockNumber"] += 1
        with self.assertRaisesRegex(ValueError, "same block"): bundle.combine(self.povw, self.zkc, "mock://rpc")

    def test_rejects_different_nonce(self):
        self.zkc["nonce"] += 1
        with self.assertRaisesRegex(ValueError, "same block"): bundle.combine(self.povw, self.zkc, "mock://rpc")

    def test_rejects_reversed_povw_calls(self):
        self.povw["data"] = simulate.encode("multiSend(bytes)", self.pack(list(reversed(self.calls[:2]))))
        with self.assertRaisesRegex(ValueError, "Accounting then Mint"): bundle.combine(self.povw, self.zkc, "mock://rpc")

    def test_rejects_initializer_payload(self):
        data = simulate.encode("upgradeToAndCall(address,bytes)", self.impls[0], "0x12345678")
        with self.assertRaisesRegex(ValueError, "empty bytes"): bundle.upgrade_implementation(data)

    def test_rejects_truncated_multisend(self):
        with self.assertRaises(ValueError): bundle.unpack_calls(self.povw["data"][:-2])

if __name__ == "__main__": unittest.main()
