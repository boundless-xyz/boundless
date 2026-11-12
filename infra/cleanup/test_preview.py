"""Check the cleanup plan stays read-only and excludes protected infrastructure."""
import copy
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location('cleanup_preview', ROOT / 'preview.py')
preview = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(preview)


class CleanupPreviewTests(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((ROOT / 'targets.json').read_text())
        self.calls = []
        self.responses = {}
        for target in self.manifest['resources']:
            for check in target['checks']:
                self.responses[(target['profile'], tuple(check['args']))] = check['expect']
        for stack in self.manifest['managed_stacks']:
            args = ('s3api', 'head-object', '--bucket', 'boundless-pulumi-state', '--key',
                    f".pulumi/stacks/{stack['project']}/{stack['stack']}.json", '--query', 'VersionId')
            self.responses[('boundless-ops', args)] = stack['version']

    def reader(self, profile, args):
        self.calls.append((profile, args))
        if args[:2] == ['sts', 'get-caller-identity']:
            return preview.ACCOUNTS[profile]
        return self.responses[(profile, tuple(args))]

    def test_preview_only_reads_and_returns_separate_deletion_steps(self):
        plan = preview.preview(self.manifest, self.reader)
        self.assertTrue(plan['read_only'])
        self.assertEqual(len(plan['resources']), len(self.manifest['resources']))
        self.assertTrue(self.calls)
        self.assertTrue(all(tuple(args[:2]) in preview.READS for _, args in self.calls))
        self.assertTrue(any('delete-' in json.dumps(x['steps']) for x in plan['resources']))

    def test_wrong_account_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'account'):
            preview.preview(self.manifest, lambda profile, args: 'wrong-account')

    def test_changed_resource_identity_is_rejected(self):
        check = self.manifest['resources'][0]['checks'][0]
        self.responses[(self.manifest['resources'][0]['profile'], tuple(check['args']))] = 'changed'
        with self.assertRaisesRegex(ValueError, 'changed'):
            preview.preview(self.manifest, self.reader)

    def test_changed_checkpoint_is_rejected(self):
        stack = self.manifest['managed_stacks'][0]
        stack['version'] = 'changed'
        with self.assertRaisesRegex(ValueError, 'checkpoint changed'):
            preview.preview(self.manifest, self.reader)

    def test_unapproved_resources_are_rejected(self):
        manifest = copy.deepcopy(self.manifest)
        manifest['resources'][0]['id'] = 'unreviewed-cleanup-target'
        with self.assertRaisesRegex(ValueError, 'Unapproved'):
            preview.validate(manifest)

    def test_protected_stack_is_rejected(self):
        self.manifest['managed_stacks'][0]['project'] = 'indexer'
        with self.assertRaisesRegex(ValueError, 'Protected stack'):
            preview.validate(self.manifest)

    def test_mutating_check_is_rejected(self):
        self.manifest['resources'][0]['checks'][0]['args'] = ['ecs', 'delete-service']
        with self.assertRaisesRegex(ValueError, 'read-only'):
            preview.validate(self.manifest)

    def test_step_cannot_target_an_unverified_resource(self):
        self.manifest['resources'][0]['steps'][0]['command'] = [
            'events', 'delete-rule', '--name', 'unreviewed-rule']
        with self.assertRaisesRegex(ValueError, 'verified target'):
            preview.validate(self.manifest)

    def test_managed_stacks_always_check_the_state_account(self):
        self.assertFalse(any(r['profile'] == 'boundless-ops' for r in self.manifest['resources']))

        def wrong_state_account(profile, args):
            if profile == 'boundless-ops':
                return 'wrong-account'
            return self.reader(profile, args)

        with self.assertRaisesRegex(ValueError, 'account'):
            preview.preview(self.manifest, wrong_state_account)


if __name__ == '__main__':
    unittest.main()
