"""Bounded, offline static route receipt checker for Aether verify.commands.

Checks only an explicit contract and its declared artifacts. It does not crawl a
site, execute JavaScript, read Aether stores, or claim deployment/cache freshness.
"""
import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path, PurePosixPath

MAX_FILES = 64
MAX_FILE_BYTES = 1024 * 1024
MAX_TOTAL_BYTES = 8 * 1024 * 1024


class Markup(HTMLParser):
    def __init__(self, text):
        super().__init__()
        self.links = set()
        self.media = set()
        self.feed(text)

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'a' and 'href' in attrs:
            self.links.add(attrs['href'])
        if tag in {'img', 'source', 'video', 'audio'} and 'src' in attrs:
            self.media.add(attrs['src'])
        if tag == 'video' and 'poster' in attrs:
            self.media.add(attrs['poster'])


def check(root, contract_path):
    root = Path(root)
    hashes, contents, findings = {}, {}, []
    consumed_bytes = 0

    def finding(code, path, detail):
        findings.append({'code': code, 'path': path, 'detail': detail})

    def read(name):
        nonlocal consumed_bytes
        if not isinstance(name, str) or not name or '\\' in name:
            raise ValueError('artifact path must be a nonempty relative POSIX path')
        parts = PurePosixPath(name)
        if parts.is_absolute() or '..' in parts.parts:
            raise ValueError(f'artifact path escapes root: {name}')
        path = (root / name).resolve(strict=True)
        path.relative_to(root)
        if name in hashes:
            return contents[name]
        if len(hashes) >= MAX_FILES:
            raise ValueError('declared artifact count exceeds limit')
        allowed = min(MAX_FILE_BYTES, max(0, MAX_TOTAL_BYTES - consumed_bytes))
        with path.open('rb') as stream:
            data = stream.read(allowed + 1)
        consumed_bytes += len(data)
        if len(data) > allowed:
            raise ValueError('declared artifact bytes exceed limit')
        hashes[name] = hashlib.sha256(data).hexdigest()
        contents[name] = data
        return data

    try:
        root = root.resolve(strict=True)
        if not root.is_dir():
            raise ValueError('artifact root must be a directory')
        contract_name = Path(contract_path).resolve(strict=True).relative_to(root).as_posix()
        contract = json.loads(read(contract_name))
        if contract.get('version') != 1:
            raise ValueError('unsupported route contract version')
        routes = contract['routes']
        if not isinstance(routes, list) or not 1 <= len(routes) <= 16:
            raise ValueError('contract must contain 1 to 16 routes')
        manifest = json.loads(read(contract['manifest']))
        reserved = set()
        for route in routes:
            canonical = route['canonical']
            aliases = route.get('aliases', [])
            if not isinstance(aliases, list) or len(aliases) > 16:
                raise ValueError('route aliases exceed limit')
            for url in [canonical, *aliases]:
                if not isinstance(url, str) or not url.startswith('/') or url.startswith('//') or any(x in url for x in ['?', '#', '\\']) or '..' in PurePosixPath(url).parts:
                    raise ValueError('route URLs must be absolute local paths without query, fragment or traversal')
                if url in reserved:
                    raise ValueError(f'duplicate canonical route or alias: {url}')
                reserved.add(url)
            actual = manifest.get(canonical, {})
            if actual.get('page') != route['page']:
                finding('wrong_page_mapping', canonical, f'expected page {route["page"]}')
            expected_media = route['media']
            if not isinstance(expected_media, list) or len(expected_media) > 16:
                raise ValueError('route media exceed limit')
            if actual.get('media') != expected_media:
                finding('wrong_media_mapping', canonical, 'manifest media differs from contract')
            for alias in aliases:
                if manifest.get(alias) != {'redirect': canonical}:
                    finding('invalid_alias', alias, f'expected intentional redirect to {canonical}')
            page = Markup(read(route['page']).decode('utf-8'))
            for media in expected_media:
                if '/' + media not in page.media:
                    finding('wrong_page_media', route['page'], f'expected media /{media}')
                read(media)
            entrypoints = route['entrypoints']
            if not isinstance(entrypoints, list) or len(entrypoints) > 16:
                raise ValueError('entrypoint count exceeds limit')
            for entry in entrypoints:
                # A contract may intentionally allow a legacy alias as an entry link.
                allowed = entry.get('allowed_hrefs', [canonical])
                if not isinstance(allowed, list) or not allowed or any(url not in [canonical, *aliases] for url in allowed):
                    raise ValueError('entrypoint hrefs must name the canonical route or a declared alias')
                markup = Markup(read(entry['path']).decode('utf-8'))
                if not markup.links.intersection(allowed):
                    finding('stale_entry_link', entry['path'], f'expected an approved route link: {allowed}')
        assets = contract['asset_sha256']
        if not isinstance(assets, dict) or len(assets) > 32:
            raise ValueError('asset receipt count exceeds limit')
        for route in routes:
            if any(media not in assets for media in route['media']):
                raise ValueError('every route media file requires a pinned SHA-256 receipt')
        for name, expected in assets.items():
            if not isinstance(expected, str) or len(expected) != 64 or any(c not in '0123456789abcdef' for c in expected):
                raise ValueError('asset receipts require lowercase SHA-256')
            read(name)
            if hashes[name] != expected:
                finding('cached_asset_mismatch', name, 'artifact bytes disagree with the pinned receipt')
    except (OSError, ValueError, TypeError, KeyError, AttributeError) as error:
        finding('invalid_or_missing_artifact', str(contract_path), str(error))
    return {'ok': not findings, 'findings': findings,
            'artifact_sha256': hashlib.sha256(json.dumps(hashes, sort_keys=True, separators=(',', ':')).encode()).hexdigest(),
            'file_sha256': hashes, 'files_read': len(hashes), 'bytes_read': consumed_bytes,
            'coverage': 'Declared local static files only. Artifact hash is separate from Aether source_hash. No browser, CDN, deployed-byte or runtime freshness assertion.'}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, required=True)
    parser.add_argument('--contract', type=Path, required=True)
    args = parser.parse_args(argv)
    result = check(args.root, args.contract)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result['ok'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
