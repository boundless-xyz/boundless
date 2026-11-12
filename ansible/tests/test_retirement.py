"""Check that the old deployment entry points cannot restart retired Bento hosts."""
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
        self.assertNotIn('pre_tasks', play)
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


if __name__ == '__main__':
    unittest.main()
