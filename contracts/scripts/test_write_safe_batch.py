import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

from write_safe_batch import build_batch


class SafeBatchTests(unittest.TestCase):
    def batch(self, targets, data=None, labels=None):
        return build_batch(1, "safe", targets, ["0xaa", "0xbb"] if data is None else data,
                           ["first", "second"] if labels is None else labels, "upgrade")

    def test_existing_single_target_calls_are_unchanged(self):
        batch = self.batch(["router"])
        self.assertEqual([call["to"] for call in batch["transactions"]], ["router", "router"])
        self.assertEqual(batch["chainId"], "1")
        self.assertEqual(batch["meta"]["createdFromSafeAddress"], "safe")

    def test_multiple_targets_preserve_call_order_and_bytes(self):
        batch = self.batch(["accounting", "mint", "zkc"], ["0xaa", "0xbb", "0xcc"], ["accounting", "mint", "zkc"])
        self.assertEqual([(call["to"], call["data"]) for call in batch["transactions"]],
                         [("accounting", "0xaa"), ("mint", "0xbb"), ("zkc", "0xcc")])
        self.assertTrue(all(call["value"] == "0" for call in batch["transactions"]))

    def test_rejects_missing_or_extra_targets(self):
        for targets in ([], ["one", "two", "three"]):
            with self.assertRaisesRegex(ValueError, "one --to"):
                self.batch(targets)

    def test_rejects_missing_labels_and_empty_batches(self):
        with self.assertRaisesRegex(ValueError, "matching --label"):
            self.batch(["router"], labels=["one"])
        with self.assertRaisesRegex(ValueError, "no calls"):
            self.batch(["router"], data=[], labels=[])

    def test_cli_writes_multi_target_json(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "ethereum.json"
            subprocess.run([sys.executable, str(Path(__file__).with_name("write_safe_batch.py")),
                            "--chain-id", "1", "--safe", "safe", "--name", "upgrade", "--output", str(path),
                            "--to", "accounting", "--data", "0xaa", "--label", "accounting",
                            "--to", "mint", "--data", "0xbb", "--label", "mint"], check=True, capture_output=True)
            self.assertEqual([call["to"] for call in json.loads(path.read_text())["transactions"]], ["accounting", "mint"])


if __name__ == "__main__":
    unittest.main()
