#!/usr/bin/env python3
"""Check retirement boundaries without AWS or Pulumi state changes."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("retirement", ROOT / "prepare-retirement.py")
retirement = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(retirement)


class RetirementTests(unittest.TestCase):
    def run_post_deploy(self, config):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "bin").mkdir()
            (root / "redshift-migrations").mkdir()
            hook = root / "post-deploy.sh"
            hook.write_text((ROOT / "order-stream/post-deploy.sh").read_text())
            log = root / "calls.jsonl"
            pulumi = root / "bin/pulumi"
            pulumi.write_text(f"#!{sys.executable}\n" + """
import json, os, sys
with open(os.environ['TEST_CALLS'], 'a') as log:
    log.write(json.dumps(sys.argv[1:]) + '\\n')
if '--json' in sys.argv:
    print(os.environ['TEST_CONFIG'])
else:
    print('synthetic-password')
""")
            pulumi.chmod(0o700)
            (root / "bin/python3").symlink_to(sys.executable)
            for name in ("psql", "envsubst", "apt-get"):
                command = root / "bin" / name
                command.write_text("#!/bin/sh\nexit 99\n")
                command.chmod(0o700)
            migration = root / "redshift-migrations/apply.sh"
            migration.write_text('#!/bin/sh\nprintf \'["migration"]\\n\' >> "$TEST_CALLS"\n')
            migration.chmod(0o700)
            env = dict(os.environ, TEST_CALLS=str(log), TEST_CONFIG=config,
                       PATH=str(root / "bin") + os.pathsep + os.environ["PATH"])
            env.pop("REDSHIFT_ADMIN_PASSWORD", None)
            env.pop("REDSHIFT_READONLY_PASSWORD", None)
            result = subprocess.run(
                ["bash", str(hook), "l-staging-84532"], env=env,
                capture_output=True, text=True, timeout=10,
            )
            calls = [json.loads(line) for line in log.read_text().splitlines()]
            return result, calls

    def test_retired_post_deploy_skips_secrets_and_migrations(self):
        for value in (True, "true"):
            result, calls = self.run_post_deploy(json.dumps({"order-stream:RETIRED": {"value": value}}))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("skipping Redshift migrations", result.stdout)
            self.assertEqual(len(calls), 1)
            self.assertEqual(calls[0][:2], ["config", "--json"])

    def test_active_post_deploy_still_runs_migrations(self):
        for config in ({}, {"order-stream:RETIRED": {"value": "false"}}):
            result, calls = self.run_post_deploy(json.dumps(config))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(calls[-1], ["migration"])
            self.assertEqual(sum(call[:2] == ["config", "get"] for call in calls), 2)

    def test_unreadable_post_deploy_config_stops_before_secrets(self):
        result, calls = self.run_post_deploy("invalid JSON")
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(len(calls), 1)

    def responses(self, service, enabled=True, indexer=None, project=None):
        values = [{f"{service}:RETIRED": {"value": str(enabled).lower()}}]
        values.append({"deployment": {"resources": [{
            "urn": f"urn:pulumi:{self.stack(service)}::{project or service}::pulumi:pulumi:Stack::stack",
            "type": "pulumi:pulumi:Stack",
        }]}})
        if service == "order-stream":
            values.append(indexer if indexer is not None else {"ORDER_STREAM_ENABLED": False})
        return [json.dumps(value) for value in values]

    def stack(self, service):
        return {"requestor-lists": "l-prod", "cw-monitoring": "production"}.get(service, "l-prod-8453")

    def test_preserved_services_are_never_touched(self):
        for service in ("indexer", "bootstrap", "pipelines"):
            with patch.object(retirement.subprocess, "check_output") as read, \
                    patch.object(retirement.subprocess, "run") as mutate:
                self.assertFalse(retirement.prepare(service, "l-prod-8453"))
                read.assert_not_called()
                mutate.assert_not_called()

    def test_disabled_retirement_leaves_state_protected(self):
        with patch.object(retirement.subprocess, "check_output", side_effect=self.responses("slasher", False)), \
                patch.object(retirement.subprocess, "run") as mutate:
            self.assertFalse(retirement.prepare("slasher", "l-prod-8453"))
            mutate.assert_not_called()

    def test_each_dedicated_service_can_retire_its_own_stack(self):
        for service in retirement.RETIRABLE_SERVICES:
            with patch.object(retirement.subprocess, "check_output", side_effect=self.responses(service)), \
                    patch.object(retirement.subprocess, "run") as mutate:
                self.assertTrue(retirement.prepare(service, self.stack(service)))
                self.assertEqual(mutate.call_count, 1)
                self.assertEqual(mutate.call_args.args[0][:5],
                                 ["pulumi", "stack", "import", "--stack", self.stack(service)])

    def test_live_or_unverified_indexer_blocks_order_stream_retirement(self):
        for indexer in ({}, {"ORDER_STREAM_ENABLED": True}):
            with patch.object(retirement.subprocess, "check_output", side_effect=self.responses("order-stream", indexer=indexer)), \
                    patch.object(retirement.subprocess, "run") as mutate:
                with self.assertRaisesRegex(ValueError, "onchain-only indexer"):
                    retirement.prepare("order-stream", "l-prod-8453")
                mutate.assert_not_called()

    def test_wrong_project_is_not_unprotected(self):
        with patch.object(retirement.subprocess, "check_output", side_effect=self.responses("slasher", project="indexer")), \
                patch.object(retirement.subprocess, "run") as mutate:
            with self.assertRaisesRegex(ValueError, "another project"):
                retirement.prepare("slasher", "l-prod-8453")
            mutate.assert_not_called()

    def test_wrong_stack_is_not_unprotected(self):
        with patch.object(retirement.subprocess, "check_output", side_effect=self.responses("slasher")), \
                patch.object(retirement.subprocess, "run") as mutate:
            with self.assertRaisesRegex(ValueError, "named service stacks"):
                retirement.prepare("slasher", "services-prod")
            mutate.assert_not_called()

    def test_retired_programs_create_no_resources_or_image_lookups(self):
        for service in retirement.RETIRABLE_SERVICES:
            result = subprocess.run(
                ["node", str(ROOT / "test-retirement.cjs"), service],
                env=dict(os.environ, NODE_PATH=str(ROOT / service / "node_modules")),
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_order_stream_program_also_checks_its_indexer_dependency(self):
        for state in ("indexer-enabled", "indexer-missing"):
            result = subprocess.run(
                ["node", str(ROOT / "test-retirement.cjs"), "order-stream", state],
                env=dict(os.environ, NODE_PATH=str(ROOT / "order-stream/node_modules")), capture_output=True, text=True, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("onchain-only indexer", result.stderr)

    def test_indexer_configs_have_no_retired_service_dependencies(self):
        for config in (ROOT / "indexer").glob("Pulumi.l-*.yaml"):
            text = config.read_text()
            for key in ("ORDER_STREAM_URL", "BENTO_API_URL", "BENTO_API_KEY", "RETIRED"):
                self.assertNotIn(f"indexer:{key}:", text, config.name)

    def test_cleanup_removes_retention_only_from_owned_resources(self):
        resources = []
        for kind, values in [
            ("aws:cloudwatch/logGroup:LogGroup", {"skipDestroy": True, "name": "service-logs"}),
            ("aws:redshift/cluster:Cluster", {"skipFinalSnapshot": False, "finalSnapshotIdentifier": "final"}),
        ]:
            resources.append({"urn": f"urn:pulumi:l-prod-8453::order-stream::{kind}::owned",
                              "type": kind, "protect": True, "inputs": dict(values), "outputs": dict(values)})
        result = retirement.prepare_checkpoint("order-stream", "l-prod-8453", {"deployment": {"resources": resources}})
        for resource in result["deployment"]["resources"]:
            self.assertFalse(resource["protect"])
        for field in ("inputs", "outputs"):
            self.assertFalse(resources[0][field]["skipDestroy"])
            self.assertEqual(resources[0][field]["name"], "service-logs")
            self.assertTrue(resources[1][field]["skipFinalSnapshot"])
            self.assertNotIn("finalSnapshotIdentifier", resources[1][field])

    def test_pending_operations_block_checkpoint_changes(self):
        with self.assertRaisesRegex(ValueError, "pending operations"):
            retirement.prepare_checkpoint("slasher", "l-prod-8453", {
                "deployment": {"pending_operations": [{}], "resources": []}})

    def test_empty_legacy_checkpoint_without_resources_is_supported(self):
        checkpoint = {"deployment": {}}
        self.assertEqual(retirement.prepare_checkpoint("slasher", "l-prod-8453", checkpoint), checkpoint)

    def test_requestor_bucket_cleanup_allows_removing_remaining_objects(self):
        bucket = {"urn": "urn:pulumi:l-prod::requestor-lists::aws:s3/bucket:Bucket::lists",
                  "type": "aws:s3/bucket:Bucket", "inputs": {}, "outputs": {}}
        retirement.prepare_checkpoint("requestor-lists", "l-prod", {"deployment": {"resources": [bucket]}})
        self.assertTrue(bucket["inputs"]["forceDestroy"])
        self.assertTrue(bucket["outputs"]["forceDestroy"])

    def test_empty_stack_removal_refuses_remaining_infrastructure(self):
        replies = self.responses("slasher")
        checkpoint = json.loads(replies[-1])
        checkpoint["deployment"]["resources"].append({
            "urn": "urn:pulumi:l-prod-8453::slasher::aws:ecs/cluster:Cluster::remaining",
            "type": "aws:ecs/cluster:Cluster",
        })
        replies[-1] = json.dumps(checkpoint)
        with patch.object(retirement.subprocess, "check_output", side_effect=replies), \
                patch.object(retirement.subprocess, "run") as mutate:
            with self.assertRaisesRegex(ValueError, "still owns resources"):
                retirement.prepare("slasher", "l-prod-8453", remove_empty=True)
            mutate.assert_not_called()

    def test_empty_stack_can_be_removed_after_retirement(self):
        with patch.object(retirement.subprocess, "check_output", side_effect=self.responses("slasher")), \
                patch.object(retirement.subprocess, "run") as mutate:
            self.assertTrue(retirement.prepare("slasher", "l-prod-8453", remove_empty=True))
            self.assertEqual(mutate.call_args.args[0],
                             ["pulumi", "stack", "rm", "--stack", "l-prod-8453", "--yes", "--non-interactive"])

    def test_legacy_and_launch_configs_are_retired(self):
        for service in retirement.RETIRABLE_SERVICES:
            for config in (ROOT / service).glob("Pulumi.*.yaml"):
                stack = config.name[7:-5]
                if stack in ("dev",) or stack.startswith("dev-"):
                    continue
                self.assertIn(f"{service}:RETIRED: true", config.read_text(), str(config))

    def test_docker_builds_keep_indexer_and_drop_retired_services(self):
        workflow = (ROOT.parent / ".github/workflows/docker-services.yml").read_text()
        self.assertIn("- name: indexer", workflow)
        for service in ("slasher", "distributor", "order-stream", "order-generator"):
            self.assertNotIn(f"- name: {service}\n", workflow)

    def test_pipeline_cleanup_preserves_indexer_and_shared_infrastructure(self):
        program = (ROOT / "pipelines/index.ts").read_text()
        for name in ("LIndexerPipeline", "CodePipelineSharedResources", "PulumiStateBucket", "PulumiSecrets", "Notifications"):
            self.assertIn(f"new {name}(", program)
        for name in ("LDistributorPipeline", "LSlasherPipeline", "LOrderStreamPipeline", "LOrderGeneratorPipeline", "LRequestorListsPipeline", "LProverAnsiblePipeline", "LKailuaBatchPipeline"):
            self.assertNotIn(f"new {name}(", program)


if __name__ == "__main__":
    unittest.main()
