import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "prepare_staging", Path(__file__).with_name("prepare.py")
)
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class PreparationGuards(unittest.TestCase):
    def test_loopback_preparation_still_forbids_mutation(self):
        rpc = prepare.ReadOnlyRPC("http://127.0.0.1:8545")
        with patch.object(prepare.staging.urllib.request, "urlopen") as network:
            for method in [
                "eth_sendTransaction",
                "eth_sendRawTransaction",
                "anvil_setBalance",
                "anvil_impersonateAccount",
            ]:
                with self.subTest(method=method), self.assertRaisesRegex(
                    AssertionError, "forbids RPC mutations"
                ):
                    rpc.call(method, [])
            network.assert_not_called()

    def test_all_state_reads_use_one_snapshot(self):
        rpc = prepare.ReadOnlyRPC("https://example.com")
        rpc.block = "0x123"
        cases = [
            ("eth_call", [{"to": "0x01"}, "latest"]),
            ("eth_getCode", ["0x01", "latest"]),
            ("eth_getStorageAt", ["0x01", "0x00", "latest"]),
        ]
        with patch.object(prepare.staging.RPC, "call", return_value="0x") as transport:
            for method, params in cases:
                rpc.call(method, params)
                self.assertEqual(
                    transport.call_args.args, (method, params[:-1] + ["0x123"])
                )
                self.assertEqual(params[-1], "latest")

    def test_mixed_block_reads_fail_before_network(self):
        rpc = prepare.ReadOnlyRPC("https://example.com")
        rpc.block = "0x123"
        with self.assertRaisesRegex(AssertionError, "mixed snapshot"):
            rpc.call("eth_getCode", ["0x01", "0x124"])

    def test_wrong_candidate_set_fails_before_network(self):
        with patch.object(prepare.staging.urllib.request, "urlopen") as network:
            with self.assertRaisesRegex(AssertionError, "exactly the candidates"):
                prepare.prepare(
                    "ethereum",
                    "https://example.com",
                    {"zkc": "0x" + "11" * 20},
                    Path("/unused"),
                )
            network.assert_not_called()

    def test_zero_candidate_fails_before_network(self):
        with patch.object(prepare.staging.urllib.request, "urlopen") as network:
            with self.assertRaisesRegex(AssertionError, "invalid candidate"):
                prepare.prepare(
                    "base",
                    "https://example.com",
                    {"base": "0x" + "00" * 20},
                    Path("/unused"),
                )
            network.assert_not_called()

    def test_duplicate_candidate_keys_rejected(self):
        with self.assertRaisesRegex(AssertionError, "duplicate candidate"):
            prepare.parse_candidates(["base=0x01", "base=0x02"])


class CandidateValidation(unittest.TestCase):
    """Exercise real validation and encoding against a deterministic RPC transport."""

    def setUp(self):
        import hashlib
        import json
        import tempfile

        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.output = self.root / "new-output/transaction.json"
        self.old = "0x" + "11" * 20
        self.candidate = "0x" + "22" * 20
        self.proxy = "0x" + "33" * 20
        self.router = "0x" + "44" * 20
        self.word = "0x" + self.router[2:].rjust(64, "0")
        self.artifact = {
            "bytecode": {"object": "0x6000"},
            "ast": {"nodes": [{"id": 1, "name": "ROUTER"}]},
            "deployedBytecode": {
                "object": "0x" + "00" * 64 + "6000a1647465737401",
                "immutableReferences": {
                    "1": [{"start": 0, "length": 32}],
                    "2": [{"start": 32, "length": 32}],
                },
            },
        }
        artifact_path = self.root / "artifact.json"
        artifact_path.write_text(json.dumps(self.artifact))
        self.artifact_hash = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
        self.runtime = (
            self.word + self.candidate[2:].rjust(64, "0") + "6000a1647465737401"
        )
        self.baseline_code = "0x6000"
        keys = ["base", "taiko", "accounting", "mint", "zkc"]
        self.manifest = {
            "artifacts": {
                key: {
                    "path": "artifact.json",
                    "creation_bytecode_keccak256": prepare.staging.keccak("0x6000"),
                    "runtime_template_keccak256": prepare.staging.keccak(
                        self.artifact["deployedBytecode"]["object"]
                    ),
                }
                for key in keys
            },
            "targets": [
                {
                    "environment": "rehearsal",
                    "contract": key,
                    "proxy": self.proxy,
                    "baseline_implementation": self.old,
                    "baseline_code_hash": prepare.staging.keccak(self.baseline_code),
                    "constructor_inputs": [{"name": "router", "value": self.router}],
                }
                for key in keys
            ],
        }
        manifest_path = self.root / "baselines.json"
        manifest_path.parent.mkdir(parents=True, exist_ok=True)
        self.save_manifest = lambda: manifest_path.write_text(json.dumps(self.manifest))
        self.save_manifest()
        fixture = self

        class FixtureRPC(prepare.ReadOnlyRPC):
            def __init__(self, url):
                super().__init__(url)
                self.chain = 84532
                self.implementation = fixture.old
                self.baseline_code = fixture.baseline_code
                self.runtime = fixture.runtime
                self.admin = 1
                self.owner = prepare.staging.SAFE
                self.threshold = 3
                self.owners = ["0x" + f"{i:040x}" for i in range(1, 7)]
                self.reorganized = False
                self.timestamp = 1793836799

            def call(self, method, params):
                assert method in prepare.staging.READS
                if method == "eth_chainId":
                    return hex(self.chain)
                if method == "eth_getBlockByNumber":
                    return {
                        "number": "0x123",
                        "timestamp": hex(self.timestamp),
                        "hash": "0x"
                        + ("bb" if self.reorganized and params[0] != "latest" else "aa")
                        * 32,
                    }
                if method == "eth_getStorageAt":
                    return "0x" + self.implementation[2:].rjust(64, "0")
                if method == "eth_getCode":
                    return (
                        self.baseline_code
                        if params[0].lower() == fixture.old
                        else self.runtime
                    )
                raise AssertionError("unexpected RPC method: " + method)

            def view(self, target, signature, *args, **kwargs):
                if signature == "ROUTER()":
                    return fixture.word
                if signature == "owner()":
                    return "0x" + self.owner[2:].rjust(64, "0")
                if signature == "hasRole(bytes32,address)":
                    return hex(self.admin)
                if signature == "getThreshold()":
                    return hex(self.threshold)
                if signature == "getOwners()":
                    return (
                        "0x"
                        + f"{32:064x}{len(self.owners):064x}"
                        + "".join(o[2:].rjust(64, "0") for o in self.owners)
                    )
                if signature == "nonce()":
                    return "0x7"
                if signature.startswith("getTransactionHash("):
                    return "0x" + "cc" * 32
                raise AssertionError("unexpected view: " + signature)

        self.rpc = FixtureRPC("https://example.com")
        self.root_patch = patch.multiple(
            prepare.staging, MARKET=self.root, ZKC=self.root, MANIFEST=manifest_path
        )
        self.transport_patch = patch.object(
            prepare, "ReadOnlyRPC", return_value=self.rpc
        )
        self.root_patch.start()
        self.addCleanup(self.root_patch.stop)
        self.transport_patch.start()
        self.addCleanup(self.transport_patch.stop)

    def run_prepare(self, label="base", candidate=None):
        return prepare.prepare(
            label,
            "https://example.com",
            {
                key: candidate or self.candidate
                for key in prepare.staging.CHAINS[label][2]
            },
            self.output,
        )

    def reject(self, message, label="base", candidate=None):
        # A rejected refresh must preserve both artifacts the signer reviewed.
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.output.write_text("previous reviewed draft")
        builder = self.output.with_name("safe-builder.json")
        builder.write_text("previous reviewed builder")
        with self.assertRaisesRegex(AssertionError, message):
            self.run_prepare(label, candidate)
        self.assertEqual(self.output.read_text(), "previous reviewed draft")
        self.assertEqual(builder.read_text(), "previous reviewed builder")
        self.assertEqual(
            sorted(p.name for p in self.output.parent.iterdir()),
            ["safe-builder.json", "transaction.json"],
        )

    def test_manifest_alone_prepares_without_prior_transaction_fixture(self):
        import json

        transaction = self.run_prepare()
        self.assertEqual(
            transaction["steps"][0]["baselineCodeHash"],
            prepare.staging.keccak(self.baseline_code),
        )
        self.assertTrue(transaction["rpc_reads_only"])
        self.assertFalse(transaction["live_submission_ready"])
        self.assertTrue(self.output.is_file())
        builder = json.loads(self.output.with_name("safe-builder.json").read_text())
        self.assertEqual(builder["transactions"][0]["to"], transaction["to"])
        self.assertEqual(builder["transactions"][0]["data"], transaction["data"])

    def test_cutoff_reached_preserves_previous_draft(self):
        for timestamp in [1793836800, 1793836801]:
            with self.subTest(timestamp=timestamp):
                self.rpc.timestamp = timestamp
                self.reject("cutoff already reached")

    def test_wrong_chain(self):
        self.rpc.chain = 1
        self.reject("wrong RPC chain")

    def test_changed_baseline_address(self):
        self.rpc.implementation = "0x" + "55" * 20
        self.reject("baseline changed")

    def test_changed_baseline_full_runtime(self):
        self.rpc.baseline_code = "0x6001"
        self.reject("baseline runtime changed")

    def test_missing_baseline_hash_fails_closed(self):
        del self.manifest["targets"][0]["baseline_code_hash"]
        self.save_manifest()
        self.output.parent.mkdir(parents=True)
        self.output.write_text("previous reviewed draft")
        with self.assertRaises(KeyError):
            self.run_prepare()
        self.assertEqual(self.output.read_text(), "previous reviewed draft")

    def test_baseline_cannot_be_candidate(self):
        self.reject("candidate is the baseline", candidate=self.old)

    def test_missing_candidate_code(self):
        self.rpc.runtime = "0x"
        self.reject("runtime/dependency mismatch")

    def test_changed_candidate_body(self):
        self.rpc.runtime = self.runtime[:-18] + "6001a1647465737401"
        self.reject("runtime/dependency mismatch")

    def test_changed_candidate_metadata(self):
        self.rpc.runtime = self.runtime[:-2] + "02"
        self.reject("runtime/dependency mismatch")

    def test_wrong_candidate_dependency(self):
        self.rpc.runtime = "0x" + "55" * 32 + self.runtime[66:]
        self.reject("runtime/dependency mismatch")

    def test_wrong_candidate_self_immutable(self):
        self.rpc.runtime = self.runtime[:66] + "55" * 32 + self.runtime[130:]
        self.reject("runtime/dependency mismatch")

    def test_artifact_changed(self):
        (self.root / "artifact.json").write_text(
            '{"bytecode": {"object": "0x6001"}, "deployedBytecode": {"object": "0x"}}'
        )
        self.reject("artifact creation bytecode changed")

    def test_manifest_dependency_changed(self):
        self.manifest["targets"][0]["constructor_inputs"][0]["value"] = self.old
        self.save_manifest()
        self.reject("constructor dependency changed")

    def test_missing_safe_admin(self):
        self.rpc.admin = 0
        self.reject("missing Safe admin")

    def test_wrong_ownable_admin(self):
        self.rpc.chain = 11155111
        self.rpc.owner = self.old
        self.reject("unexpected Safe owner", label="ethereum")

    def test_wrong_threshold(self):
        self.rpc.threshold = 2
        self.reject("unexpected Safe threshold")

    def test_wrong_owner_count(self):
        self.rpc.owners.pop()
        self.reject("unexpected Safe threshold")

    def test_duplicate_owners(self):
        self.rpc.owners[-1] = self.rpc.owners[0]
        self.reject("invalid Safe owners")

    def test_zero_owner(self):
        self.rpc.owners[-1] = prepare.staging.ZERO
        self.reject("invalid Safe owners")

    def test_snapshot_reorganization_preserves_previous_draft(self):
        self.rpc.reorganized = True
        self.reject("snapshot reorganized")

    def test_taiko_draft_warns_about_shared_production_nonce(self):
        self.rpc.chain = 167000
        transaction = self.run_prepare("taiko")
        self.assertEqual(transaction["safe"], prepare.staging.TAIKO_SAFE)
        self.assertIn("share this Safe and nonce", transaction["scope"])
        self.assertIn("regenerate the production draft", transaction["scope"])

    def test_production_configuration_uses_reviewed_production_targets(self):
        for target in self.manifest["targets"]:
            target["environment"] = "production"
        self.save_manifest()
        self.rpc.chain = 8453
        with patch.object(prepare.staging, "ENVIRONMENT", "production"), patch.object(
            prepare.staging, "CHAINS", prepare.staging.PRODUCTION_CHAINS
        ):
            transaction = self.run_prepare()
        self.assertEqual(transaction["chainId"], 8453)
        self.assertEqual(transaction["steps"][0]["proxy"], self.proxy)
        self.assertFalse(transaction["live_submission_ready"])

    def test_changed_runtime_template_rejected_before_candidate_comparison(self):
        import json

        self.artifact["deployedBytecode"]["object"] += "00"
        (self.root / "artifact.json").write_text(json.dumps(self.artifact))
        self.reject("artifact runtime template changed")


if __name__ == "__main__":
    unittest.main()
