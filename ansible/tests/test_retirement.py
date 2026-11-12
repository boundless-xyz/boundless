"""Check that the old deployment entry points cannot restart retired Bento hosts."""
import copy
import os
import shutil
import socket
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[1]


class RetirementTests(unittest.TestCase):
    def test_pipeline_entrypoint_only_imports_retirement(self):
        plays = yaml.safe_load((ROOT / 'prover.yml').read_text())
        self.assertEqual(len(plays), 1)
        self.assertEqual(plays[0]['ansible.builtin.import_playbook'], 'retire-bento.yml')

    def test_old_setup_entrypoints_do_not_change_hosts(self):
        for name in ('monitoring', 'security'):
            play = yaml.safe_load((ROOT / f'{name}.yml').read_text())[0]
            self.assertFalse(play['gather_facts'])
            self.assertFalse(play['become'])
            self.assertTrue(all('ansible.builtin.debug' in task for task in play['tasks']))

    def test_retirement_stops_workloads_without_an_execution_guard(self):
        play = yaml.safe_load((ROOT / 'retire-bento.yml').read_text())[0]
        self.assertEqual(play['hosts'], 'all')
        self.assertFalse(play.get('ignore_unreachable', False))
        self.assertFalse(play.get('ignore_errors', False))
        stop = next(t for t in play['tasks'] if 'ansible.builtin.systemd_service' in t)
        self.assertEqual(stop['ansible.builtin.systemd_service']['state'], 'stopped')
        self.assertFalse(stop['ansible.builtin.systemd_service']['enabled'])
        self.assertEqual(stop['loop'], ['bento.service', 'vector.service'])
        commands = [t['ansible.builtin.command']['argv'] for t in play['tasks']
                    if 'ansible.builtin.command' in t]
        self.assertTrue(all(command[0] == 'docker' for command in commands))
        remove = next(command for command in commands if 'rm' in command)
        self.assertNotIn('--volumes', remove)
        self.assertNotIn('-v', remove)


@unittest.skipUnless(shutil.which('ansible-playbook'), 'ansible-playbook is required')
class RetirementConnectionTests(unittest.TestCase):
    def run_playbook(self, directory, playbook, port, connection='ssh', inventory_hosts=None):
        inventory = Path(directory) / 'inventory.yml'
        hosts = inventory_hosts or {'retired_test_host': {
            'ansible_host': '127.0.0.1',
            'ansible_port': port,
            'ansible_connection': connection,
            'ansible_connect_timeout': 1,
        }}
        inventory.write_text(yaml.safe_dump({'all': {'hosts': hosts}}))
        env = dict(os.environ, ANSIBLE_LOCAL_TEMP=str(Path(directory) / 'tmp'),
                   ANSIBLE_HOST_KEY_CHECKING='False', ANSIBLE_NOCOLOR='1')
        return subprocess.run(
            ['ansible-playbook', '-i', str(inventory), str(playbook), '--check'],
            env=env, capture_output=True, text=True, timeout=40,
        )

    def test_unreachable_host_does_not_block_retirement_pipeline(self):
        with tempfile.TemporaryDirectory() as directory, socket.socket() as unavailable:
            unavailable.bind(('127.0.0.1', 0))
            result = self.run_playbook(directory, ROOT / 'prover.yml', unavailable.getsockname()[1])
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('Skipping unreachable retired host retired_test_host', result.stdout)

    def test_reachable_host_cleanup_errors_still_fail(self):
        play = copy.deepcopy(yaml.safe_load((ROOT / 'retire-bento.yml').read_text())[0])
        play['become'] = False
        play['tasks'] = [{'name': 'Simulate a cleanup failure',
                          'ansible.builtin.fail': {'msg': 'retirement cleanup failed'}}]
        play.pop('handlers', None)
        with tempfile.TemporaryDirectory() as directory, socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            available.listen()
            fixture = Path(directory) / 'cleanup-failure.yml'
            fixture.write_text(yaml.safe_dump([play]))
            result = self.run_playbook(directory, fixture, available.getsockname()[1], 'local')
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('retirement cleanup failed', result.stdout)
        self.assertNotIn('Skipping unreachable retired host', result.stdout)

    def test_delegated_probe_uses_original_inventory_address(self):
        play = copy.deepcopy(yaml.safe_load((ROOT / 'retire-bento.yml').read_text())[0])
        play['hosts'] = '127.0.0.1'
        play['become'] = False
        play['tasks'] = [{'name': 'Confirm the reachable host was not skipped',
                          'ansible.builtin.fail': {'msg': 'original host reached'}}]
        play.pop('handlers', None)
        with tempfile.TemporaryDirectory() as directory, socket.socket() as available:
            available.bind(('127.0.0.1', 0))
            available.listen()
            port = available.getsockname()[1]
            hosts = {
                '127.0.0.1': {'ansible_port': port, 'ansible_connection': 'local'},
                'localhost': {'ansible_host': '127.0.0.2', 'ansible_connection': 'local'},
            }
            fixture = Path(directory) / 'inventory-address.yml'
            fixture.write_text(yaml.safe_dump([play]))
            result = self.run_playbook(directory, fixture, port, inventory_hosts=hosts)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('original host reached', result.stdout)
        self.assertNotIn('Skipping unreachable retired host', result.stdout)


if __name__ == '__main__':
    unittest.main()
