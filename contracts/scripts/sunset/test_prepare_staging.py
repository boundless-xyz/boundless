import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('prepare_staging', Path(__file__).with_name('prepare-staging.py'))
prepare = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prepare)


class PreparationGuards(unittest.TestCase):
    def test_loopback_preparation_still_forbids_mutation(self):
        rpc = prepare.ReadOnlyRPC('http://127.0.0.1:8545')
        with patch.object(prepare.staging.urllib.request, 'urlopen') as network:
            for method in ['eth_sendTransaction', 'eth_sendRawTransaction', 'anvil_setBalance', 'anvil_impersonateAccount']:
                with self.subTest(method=method), self.assertRaisesRegex(AssertionError, 'forbids RPC mutations'):
                    rpc.call(method, [])
            network.assert_not_called()

    def test_all_state_reads_use_one_snapshot(self):
        rpc = prepare.ReadOnlyRPC('https://example.com')
        rpc.block = '0x123'
        cases = [('eth_call', [{'to': '0x01'}, 'latest']), ('eth_getCode', ['0x01', 'latest']),
                 ('eth_getStorageAt', ['0x01', '0x00', 'latest'])]
        with patch.object(prepare.staging.RPC, 'call', return_value='0x') as transport:
            for method, params in cases:
                rpc.call(method, params)
                self.assertEqual(transport.call_args.args, (method, params[:-1] + ['0x123']))
                self.assertEqual(params[-1], 'latest')

    def test_mixed_block_reads_fail_before_network(self):
        rpc = prepare.ReadOnlyRPC('https://example.com')
        rpc.block = '0x123'
        with self.assertRaisesRegex(AssertionError, 'mixed snapshot'):
            rpc.call('eth_getCode', ['0x01', '0x124'])

    def test_wrong_candidate_set_fails_before_network(self):
        with patch.object(prepare.staging.urllib.request, 'urlopen') as network:
            with self.assertRaisesRegex(AssertionError, 'exactly the candidates'):
                prepare.prepare('ethereum', 'https://example.com', {'zkc': '0x' + '11' * 20}, Path('/unused'))
            network.assert_not_called()

    def test_zero_candidate_fails_before_network(self):
        with patch.object(prepare.staging.urllib.request, 'urlopen') as network:
            with self.assertRaisesRegex(AssertionError, 'invalid candidate'):
                prepare.prepare('base', 'https://example.com', {'base': '0x' + '00' * 20}, Path('/unused'))
            network.assert_not_called()

    def test_duplicate_candidate_keys_rejected(self):
        with self.assertRaisesRegex(AssertionError, 'duplicate candidate'):
            prepare.parse_candidates(['base=0x01', 'base=0x02'])


if __name__ == '__main__':
    unittest.main()
