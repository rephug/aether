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

    def test_same_page_alias_with_canonical_is_valid(self):
        self.contract['routes'][0]['alias_modes'] = {'/realtor': 'same_page'}
        self.manifest['/realtor'] = {'page': 'agents.html', 'canonical': '/agents'}
        self.write('agents.html', '<link rel="canonical" href="/agents"><img src="/assets/agents.svg">')
        self.assertTrue(self.run_check()['ok'])

    def test_good_link_does_not_hide_stale_link(self):
        self.write('index.html', '<a href="/agents">Good</a><a href="/retired-realtor">Stale</a>')
        self.assertIn('stale_entry_link', self.codes(self.run_check()))

    def test_expected_image_does_not_hide_wrong_image(self):
        self.write('agents.html', '<img src="/assets/agents.svg"><img src="/wrong.jpg">')
        self.assertIn('unexpected_page_media', self.codes(self.run_check()))

    def test_srcset_is_checked(self):
        self.write('agents.html', '<img src="/assets/agents.svg" srcset="/assets/agents.svg 1x, /wrong.jpg 2x">')
        self.assertIn('unexpected_page_media', self.codes(self.run_check()))

    def test_inline_css_media_is_checked(self):
        self.write('agents.html', '<style>.hero {background:url(/wrong.jpg)}</style><img src="/assets/agents.svg">')
        self.assertIn('unexpected_page_media', self.codes(self.run_check()))

    def test_uninterpreted_javascript_fails_closed(self):
        self.write('agents.html', '<img src="/assets/agents.svg"><script>new Image().src="/wrong.jpg"</script>')
        self.assertIn('unsupported_dynamic_content', self.codes(self.run_check()))

    def test_relative_media_and_cache_suffixes_are_normalized(self):
        self.write('agents.html', '<img src="assets/agents.svg?v=123#preview">')
        self.write('index.html', '<a href="agents?campaign=1#watch">Agents</a>')
        self.assertTrue(self.run_check()['ok'])

    def test_encoded_delimiters_do_not_turn_wrong_paths_into_valid_routes(self):
        for suffix in ['%3Fretired', '%23retired']:
            with self.subTest(suffix=suffix):
                self.write('index.html', f'<a href="/agents{suffix}">Wrong path</a>')
                self.assertFalse(self.run_check()['ok'])

    def test_encoded_delimiters_do_not_hide_wrong_media(self):
        for suffix in ['%3Fretired', '%23retired']:
            with self.subTest(suffix=suffix):
                self.write('agents.html', f'<img src="/assets/agents.svg{suffix}">')
                self.assertFalse(self.run_check()['ok'])

    def test_encoded_url_structure_and_controls_are_rejected(self):
        for href in ['%2Fagents', 'https%3A%2F%2Ffixture.invalid%2Fagents',
                     '/%61gents%0A', '/ag\tents', '/agents%7F']:
            with self.subTest(href=href):
                self.write('index.html', f'<a href="{href}">Ambiguous</a>')
                self.assertFalse(self.run_check()['ok'])

    def test_encoded_unreserved_path_bytes_are_supported(self):
        self.write('index.html', '<a href="/%61gents?campaign=1#top">Agents</a>')
        self.write('agents.html', '<img src="/assets/%61gents.svg?v=123#preview">')
        self.assertTrue(self.run_check()['ok'])

    def test_duplicate_contract_keys_fail_closed(self):
        self.write('contract.json', '{"version": 1, "version": 1}')
        result = check(self.root, self.root / 'contract.json')
        self.assertIn('duplicate JSON key', str(result))

    def test_external_css_references_are_checked_and_pinned(self):
        self.write('style.css', '.hero {background:url(wrong.jpg)}')
        self.contract['asset_sha256']['style.css'] = hashlib.sha256((self.root / 'style.css').read_bytes()).hexdigest()
        self.write('agents.html', '<link rel="stylesheet" href="/style.css"><img src="/assets/agents.svg">')
        self.assertIn('unexpected_page_media', self.codes(self.run_check()))

    def test_pinned_uninterpreted_scripts_require_page_and_script_receipts(self):
        self.contract['routes'][0]['script_policy'] = 'pinned_uninterpreted'
        self.write('script.js', '/* explicitly unverified JavaScript semantics */')
        self.write('agents.html', '<img src="/assets/agents.svg"><script src="/script.js"></script>')
        self.assertIn('invalid_or_missing_artifact', self.codes(self.run_check()))
        for name in ['agents.html', 'script.js']:
            self.contract['asset_sha256'][name] = hashlib.sha256((self.root / name).read_bytes()).hexdigest()
        self.assertTrue(self.run_check()['ok'])
        self.write('script.js', 'new Image().src="/wrong.jpg"')
        self.assertIn('cached_asset_mismatch', self.codes(self.run_check()))

    def test_explicit_unrelated_links_do_not_hide_new_stale_links(self):
        self.contract['routes'][0]['entrypoints'][0]['ignore_hrefs'] = ['/about']
        self.write('index.html', '<a href="/agents">Good</a><a href="/about">About</a>')
        self.assertTrue(self.run_check()['ok'])
        self.write('index.html', '<a href="/agents">Good</a><a href="/about">About</a><a href="/wrong-agents">Wrong</a>')
        self.assertIn('stale_entry_link', self.codes(self.run_check()))

    def test_entrypoint_scripts_also_fail_closed(self):
        self.write('index.html', '<a href="/agents">Good</a><script>location.href="/retired-realtor"</script>')
        self.assertIn('unsupported_dynamic_content', self.codes(self.run_check()))

    def test_entrypoint_script_policy_requires_pinned_entry_and_script(self):
        self.contract['routes'][0]['entrypoints'][0]['script_policy'] = 'pinned_uninterpreted'
        self.write('script.js', '/* source identity only, not interpreted */')
        self.write('index.html', '<a href="/agents">Good</a><script src="script.js"></script>')
        self.assertIn('invalid_or_missing_artifact', self.codes(self.run_check()))
        for name in ['index.html', 'script.js']:
            self.contract['asset_sha256'][name] = hashlib.sha256((self.root / name).read_bytes()).hexdigest()
        self.assertTrue(self.run_check()['ok'])

    def test_entrypoint_base_element_is_not_silently_ignored(self):
        self.write('index.html', '<base href="/wrong/"><a href="agents">Good-looking link</a>')
        self.assertIn('unsupported_static_content', self.codes(self.run_check()))

    def test_wrong_page_and_media_mapping_are_rejected(self):
        self.manifest['/agents'] = {'page': 'realtor.html', 'media': ['assets/realtor.svg']}
        self.assertEqual(self.codes(self.run_check()), {'wrong_page_mapping', 'wrong_media_mapping'})

    def test_page_media_content_is_checked(self):
        self.write('agents.html', '<img src="/assets/realtor.svg">')
        self.assertEqual(self.codes(self.run_check()), {'wrong_page_media', 'unexpected_page_media'})

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
