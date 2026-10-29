import importlib.util
import json
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
            "cutoff_timestamp": 1793836800,
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
                self.nonce = 7
                self.hash_calls = []

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
                    return hex(self.nonce)
                if signature.startswith("getTransactionHash("):
                    self.hash_calls.append(args)
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

    def run_prepare(self, label="base", candidate=None, rollback=False):
        return prepare.prepare(
            label,
            "https://example.com",
            {
                key: candidate or self.candidate
                for key in prepare.staging.CHAINS[label][2]
            },
            self.output,
            rollback=rollback,
        )

    def reject(self, message, label="base", candidate=None, rollback=False):
        # A rejected refresh must preserve the upgrade and rollback artifacts.
        self.output.parent.mkdir(parents=True, exist_ok=True)
        self.output.write_text("previous reviewed draft")
        builder = self.output.with_name("safe-builder.json")
        builder.write_text("previous reviewed builder")
        recovery = self.output.parent / "rollback"
        recovery.mkdir(exist_ok=True)
        for name in ["transaction.json", "safe-builder.json"]:
            (recovery / name).write_text("previous reviewed rollback")
        with self.assertRaisesRegex(AssertionError, message):
            self.run_prepare(label, candidate, rollback=rollback)
        self.assertEqual(self.output.read_text(), "previous reviewed draft")
        self.assertEqual(builder.read_text(), "previous reviewed builder")
        for name in ["transaction.json", "safe-builder.json"]:
            self.assertEqual((recovery / name).read_text(), "previous reviewed rollback")
        self.assertEqual(
            sorted(p.name for p in self.output.parent.iterdir()),
            ["rollback", "safe-builder.json", "transaction.json"],
        )

    def test_upgrade_exports_matching_rollback_at_next_nonce(self):
        upgrade = self.run_prepare()
        folder = self.output.parent / "rollback"
        rollback = json.loads((folder / "transaction.json").read_text())
        builder = json.loads((folder / "safe-builder.json").read_text())
        self.assertEqual(upgrade["action"], "upgrade")
        self.assertEqual(rollback["action"], "rollback")
        self.assertEqual(rollback["nonce"], upgrade["nonce"] + 1)
        self.assertEqual([args[-1] for args in self.rpc.hash_calls], [7, 8])
        for key in ["safe", "chainId", "to", "operation", "snapshotBlockHash"]:
            self.assertEqual(rollback[key], upgrade[key])
        self.assertEqual(rollback["steps"][0]["upgradedImplementation"], self.candidate)
        self.assertEqual(rollback["steps"][0]["upgradedRuntimeCodeHash"], prepare.staging.keccak(self.runtime))
        self.assertEqual(rollback["steps"][0]["runtimeCodeHash"], prepare.staging.keccak(self.baseline_code))
        # Decode the saved call independently of the exporter, including empty init bytes.
        call = bytes.fromhex(rollback["data"][2:])
        self.assertEqual(call[:4].hex(), "4f1ef286")
        self.assertEqual(call[4:36], bytes.fromhex(self.old[2:].rjust(64, "0")))
        self.assertEqual(int.from_bytes(call[36:68], "big"), 64)
        self.assertEqual(call[68:], bytes(32))
        self.assertEqual(len(call), 100)
        self.assertEqual(builder["transactions"][0]["data"], rollback["data"])
        self.assertEqual(builder["transactions"][0]["value"], "0")
        self.assertIn("Restores original PoVW", rollback["scope"])

    def test_refresh_rollback_after_cutoff_uses_current_nonce(self):
        self.rpc.implementation = self.candidate
        self.rpc.timestamp = 1793836801
        self.rpc.nonce = 10
        rollback = self.run_prepare(rollback=True)
        self.assertEqual(rollback["action"], "rollback")
        self.assertEqual(rollback["nonce"], 10)
        self.assertEqual(rollback["steps"][0]["implementation"], self.old)
        self.assertEqual([args[-1] for args in self.rpc.hash_calls], [10])
        self.assertTrue(rollback["rpc_reads_only"])
        self.assertFalse(rollback["live_submission_ready"])

    def test_rollback_requires_expected_installed_candidate(self):
        for current in [self.old, "0x" + "55" * 20]:
            with self.subTest(current=current):
                self.rpc.implementation = current
                self.reject("installed candidate changed", rollback=True)

    def test_rollback_rechecks_baseline_runtime_and_candidate_runtime(self):
        self.rpc.implementation = self.candidate
        self.rpc.baseline_code = "0x6001"
        self.reject("baseline runtime changed", rollback=True)
        self.rpc.baseline_code = self.baseline_code
        self.rpc.runtime = "0x"
        self.reject("runtime/dependency mismatch", rollback=True)

    def test_rollback_reorganization_preserves_previous_drafts(self):
        self.rpc.implementation = self.candidate
        self.rpc.reorganized = True
        self.reject("snapshot reorganized", rollback=True)

    def test_blocked_rollback_destination_preserves_upgrade(self):
        self.output.parent.mkdir(parents=True)
        self.output.write_text("previous reviewed draft")
        builder = self.output.with_name("safe-builder.json")
        builder.write_text("previous reviewed builder")
        recovery = self.output.parent / "rollback"
        recovery.write_text("blocked directory")
        with self.assertRaises(FileExistsError):
            self.run_prepare()
        self.assertEqual(self.output.read_text(), "previous reviewed draft")
        self.assertEqual(builder.read_text(), "previous reviewed builder")
        self.assertEqual(recovery.read_text(), "blocked directory")

    def test_publication_failure_restores_previous_drafts(self):
        replace = Path.replace
        for existing, failed_name in [
            (False, "safe-builder.json"), (False, "transaction.json"),
            (True, "safe-builder.json"), (True, "transaction.json"),
        ]:
            with self.subTest(existing=existing, failed_name=failed_name):
                self.output = self.root / (str(existing) + failed_name) / "transaction.json"
                if existing:
                    self.run_prepare()
                paths = [self.output, self.output.with_name("safe-builder.json")]
                paths += [self.output.parent / "rollback" / name for name in ["transaction.json", "safe-builder.json"]]
                previous = {p: p.read_bytes() if p.exists() else None for p in paths}
                self.rpc.nonce += 1

                def fail_rollback(source, destination):
                    if source.name == failed_name and Path(destination) == self.output.parent / "rollback" / failed_name:
                        raise OSError("simulated rollback publication failure")
                    return replace(source, destination)

                with patch.object(Path, "replace", fail_rollback):
                    with self.assertRaisesRegex(OSError, "simulated rollback publication failure"):
                        self.run_prepare()
                self.assertEqual({p: p.read_bytes() if p.exists() else None for p in paths}, previous)

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
        rollback = json.loads((self.output.parent / "rollback/transaction.json").read_text())
        self.assertIn("share this Safe and nonce", rollback["scope"])
        self.assertIn("any intervening Safe transaction", rollback["scope"])
        self.assertNotIn("Execute staging first", rollback["scope"])

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


class EthereumBatchExport(unittest.TestCase):
    def setUp(self):
        import tempfile
        from unittest.mock import Mock

        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.output = Path(self.temp.name) / "transaction.json"
        self.helper = "0x9641d764fc13c8B624c04430C7356C1C7C8102e2"
        self.code_hash = "0xecd5bd14a08c5d2122379900b2f272bdf107a7e92423c10dd5fe3254386c9939"
        self.rpc = Mock()
        owners = [f"{i:064x}" for i in range(1, 7)]
        self.rpc.view.side_effect = ["0x" + f"{32:064x}{6:064x}" + "".join(owners), "0x" + "cc" * 32]
        self.rpc.uint.side_effect = [3, 10]
        self.rpc.call.return_value = "0x6000"
        self.steps = [
            {"contract": key, "proxy": "0x" + f"{i:040x}", "implementation": "0x" + "22" * 20}
            for i, key in enumerate(["accounting", "mint", "zkc"], 1)
        ]

    def check_export(self, nonce):
        with patch.object(prepare.staging, "keccak", return_value=self.code_hash):
            transaction = prepare.staging.export_transaction(
                self.rpc, "ethereum", 1, self.steps, self.output, 123, nonce=nonce
            )
        self.rpc.call.assert_called_once_with("eth_getCode", [self.helper, "latest"])
        self.assertEqual(transaction["to"], self.helper)
        self.assertEqual(transaction["operation"], 1)
        self.assertEqual(transaction["nonce"], nonce)
        hash_args = self.rpc.view.call_args.args[2:]
        self.assertEqual(
            hash_args,
            (self.helper, 0, transaction["data"], 1, 0, 0, 0,
             prepare.staging.ZERO, prepare.staging.ZERO, nonce),
        )
        builder = json.loads(self.output.with_name("safe-builder.json").read_text())
        self.assertEqual([call["to"] for call in builder["transactions"]],
                         [step["proxy"] for step in self.steps])

    def test_ethereum_upgrade_hash_uses_safe_ui_helper(self):
        self.check_export(10)

    def test_ethereum_rollback_hash_uses_safe_ui_helper(self):
        self.check_export(11)

    def test_changed_helper_runtime_stops_before_hashing(self):
        with patch.object(prepare.staging, "keccak", return_value="0x" + "00" * 32):
            with self.assertRaisesRegex(AssertionError, "unexpected MultiSend runtime"):
                prepare.staging.export_transaction(
                    self.rpc, "ethereum", 1, self.steps, self.output, 123
                )
        self.assertEqual(self.rpc.view.call_count, 1)
        self.assertFalse(self.output.exists())


if __name__ == "__main__":
    unittest.main()
