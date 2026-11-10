#!/usr/bin/env python3
"""Check retirement boundaries without AWS or Pulumi state changes."""

import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("retirement", ROOT / "prepare-retirement.py")
retirement = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(retirement)


class RetirementTests(unittest.TestCase):
    def responses(self, service, enabled=True, indexer=None, project=None):
        values = [{f"{service}:RETIRED": {"value": str(enabled).lower()}}]
        if service == "order-stream":
            values.append(indexer if indexer is not None else {"ORDER_STREAM_ENABLED": False})
        values.append({"deployment": {"resources": [{
            "urn": f"urn:pulumi:l-prod-8453::{project or service}::pulumi:pulumi:Stack::stack"
        }]}})
        return [json.dumps(value) for value in values]

    def test_preserved_services_are_never_touched(self):
        for service in ("indexer", "bootstrap", "pipelines", "requestor-lists", "cw-monitoring"):
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
                self.assertTrue(retirement.prepare(service, "l-prod-8453"))
                mutate.assert_called_once_with(
                    ["pulumi", "state", "unprotect", "--all", "--yes", "--stack", "l-prod-8453"],
                    check=True,
                )

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
            with self.assertRaisesRegex(ValueError, "named launch stacks"):
                retirement.prepare("slasher", "services-prod")
            mutate.assert_not_called()

    def test_retired_programs_create_no_resources_or_image_lookups(self):
        for service in retirement.RETIRABLE_SERVICES:
            result = subprocess.run(
                ["node", str(ROOT / "test-retirement.cjs"), service],
                env=os.environ,
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 0, result.stderr)

    def test_order_stream_program_also_checks_its_indexer_dependency(self):
        for state in ("indexer-enabled", "indexer-missing"):
            result = subprocess.run(
                ["node", str(ROOT / "test-retirement.cjs"), "order-stream", state],
                env=os.environ, capture_output=True, text=True, timeout=30,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("onchain-only indexer", result.stderr)

    def test_indexer_configs_have_no_retired_service_dependencies(self):
        for config in (ROOT / "indexer").glob("Pulumi.l-*.yaml"):
            text = config.read_text()
            for key in ("ORDER_STREAM_URL", "BENTO_API_URL", "BENTO_API_KEY", "RETIRED"):
                self.assertNotIn(f"indexer:{key}:", text, config.name)


if __name__ == "__main__":
    unittest.main()
