import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from check_route_artifacts import check, MAX_FILE_BYTES, MAX_TOTAL_BYTES


class RouteArtifactsTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / 'assets').mkdir()
        self.write('assets/agents.svg', '<svg>agents</svg>')
        self.write('agents.html', '<img src="/assets/agents.svg">')
        self.write('index.html', '<a href="/agents">Agents</a>')
        self.contract = {'version': 1, 'manifest': 'route-manifest.json',
            'routes': [{'canonical': '/agents', 'aliases': ['/realtor'], 'page': 'agents.html',
                        'media': ['assets/agents.svg'], 'entrypoints': [{'path': 'index.html'}]}],
            'asset_sha256': {'assets/agents.svg': hashlib.sha256((self.root / 'assets/agents.svg').read_bytes()).hexdigest()}}
        self.manifest = {'/agents': {'page': 'agents.html', 'media': ['assets/agents.svg']},
                         '/realtor': {'redirect': '/agents'}}

    def write(self, name, text):
        (self.root / name).write_text(text)

    def run_check(self):
        self.write('contract.json', json.dumps(self.contract))
        self.write('route-manifest.json', json.dumps(self.manifest))
        return check(self.root, self.root / 'contract.json')

    def codes(self, result):
        return {x['code'] for x in result['findings']}

    def test_intentional_alias_is_valid(self):
        result = self.run_check()
        self.assertTrue(result['ok'], result)
        self.assertEqual(result['files_read'], 5)

    def test_explicitly_approved_alias_entry_is_valid(self):
        self.write('index.html', '<a href="/realtor">Agents</a>')
        self.contract['routes'][0]['entrypoints'][0]['allowed_hrefs'] = ['/agents', '/realtor']
        self.assertTrue(self.run_check()['ok'])

    def test_stale_entry_link_is_rejected(self):
        self.write('index.html', '<a href="/retired-realtor">Agents</a>')
        self.assertEqual(self.codes(self.run_check()), {'stale_entry_link'})

    def test_wrong_page_and_media_mapping_are_rejected(self):
        self.manifest['/agents'] = {'page': 'realtor.html', 'media': ['assets/realtor.svg']}
        self.assertEqual(self.codes(self.run_check()), {'wrong_page_mapping', 'wrong_media_mapping'})

    def test_page_media_content_is_checked(self):
        self.write('agents.html', '<img src="/assets/realtor.svg">')
        self.assertEqual(self.codes(self.run_check()), {'wrong_page_media'})

    def test_cache_digest_and_artifact_hash_change(self):
        before = self.run_check()
        self.write('assets/agents.svg', '<svg>cached realtor</svg>')
        after = self.run_check()
        self.assertEqual(self.codes(after), {'cached_asset_mismatch'})
        self.assertNotEqual(before['artifact_sha256'], after['artifact_sha256'])

    def test_alias_must_redirect_to_declared_canonical(self):
        self.manifest['/realtor'] = {'redirect': '/different-page'}
        self.assertEqual(self.codes(self.run_check()), {'invalid_alias'})

    def test_missing_and_unbounded_artifacts_fail(self):
        (self.root / 'agents.html').unlink()
        self.assertEqual(self.codes(self.run_check()), {'invalid_or_missing_artifact'})
        self.write('agents.html', 'x' * (MAX_FILE_BYTES + 1))
        self.assertEqual(self.codes(self.run_check()), {'invalid_or_missing_artifact'})

    def test_traversal_and_unknown_alias_fail(self):
        self.contract['routes'][0]['page'] = '../outside.html'
        self.assertIn('invalid_or_missing_artifact', self.codes(self.run_check()))
        self.contract['routes'][0]['page'] = 'agents.html'
        self.contract['routes'][0]['entrypoints'][0]['allowed_hrefs'] = ['/unknown']
        self.assertEqual(self.codes(self.run_check()), {'invalid_or_missing_artifact'})

    def test_invalid_root_returns_structured_failure(self):
        self.assertEqual(self.codes(check(self.root / 'missing', self.root / 'contract.json')), {'invalid_or_missing_artifact'})

    def test_missing_media_receipt_is_rejected(self):
        self.contract['asset_sha256'] = {}
        self.assertEqual(self.codes(self.run_check()), {'invalid_or_missing_artifact'})

    def test_symlink_outside_root_is_rejected(self):
        with tempfile.TemporaryDirectory() as outside:
            target = Path(outside) / 'private.html'
            target.write_text('<main>outside</main>')
            link = self.root / 'outside.html'
            try:
                link.symlink_to(target)
            except OSError:
                self.skipTest('symlinks unavailable on this host')
            self.contract['routes'][0]['page'] = 'outside.html'
            self.assertIn('invalid_or_missing_artifact', self.codes(self.run_check()))

    def test_total_byte_budget_stops_after_one_sentinel_byte(self):
        media = []
        for index in range(9):
            name = f'assets/{index}.svg'
            self.write(name, 'x' * MAX_FILE_BYTES)
            media.append(name)
            self.contract['asset_sha256'][name] = hashlib.sha256((self.root / name).read_bytes()).hexdigest()
        self.contract['routes'][0]['media'] = media
        self.manifest['/agents']['media'] = media
        self.write('agents.html', ''.join(f'<img src="/{name}">' for name in media))
        result = self.run_check()
        self.assertEqual(self.codes(result), {'invalid_or_missing_artifact'})
        self.assertLessEqual(result['bytes_read'], MAX_TOTAL_BYTES + 1)


if __name__ == '__main__':
    unittest.main()
