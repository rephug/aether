"""Bounded offline assertions over explicitly declared route artifacts.

Static HTML/srcset and local CSS url() references are checked exactly. JavaScript
is rejected unless explicitly accepted as pinned, uninterpreted source evidence.
This checker never executes a browser or establishes deployed/CDN behavior.
"""
import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path, PurePosixPath
import re
from urllib.parse import unquote, urljoin, urlsplit

MAX_FILES = 64
MAX_FILE_BYTES = 1024 * 1024
MAX_TOTAL_BYTES = 8 * 1024 * 1024


def css_media(text):
    if '@import' in text.lower() or '\\' in text or 'image-set(' in text.lower():
        raise ValueError('CSS imports, escapes and image-set require a dedicated verifier')
    return {match.group(2).strip() for match in re.finditer(
        r'url\(\s*([\'\"]?)(.*?)\1\s*\)', text, re.I | re.S)}


class Markup(HTMLParser):
    def __init__(self, text):
        super().__init__(convert_charrefs=True)
        self.links, self.media, self.stylesheets, self.canonicals = set(), set(), set(), set()
        self.scripts, self.problems = [], []
        self.in_style = False
        self.feed(text)
        self.close()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if tag == 'base':
            self.problems.append('HTML base URLs require a dedicated verifier')
        if any(name.startswith('on') for name in attrs):
            self.scripts.append(None)
        if tag == 'a' and attrs.get('href'):
            self.links.add(attrs['href'])
        if tag in {'img', 'source', 'video', 'audio', 'embed', 'image', 'input'}:
            for name in ['src', 'poster', 'href', 'xlink:href']:
                if attrs.get(name):
                    self.media.add(attrs[name])
            if attrs.get('srcset'):
                value = attrs['srcset']
                if 'data:' in value.lower():
                    self.problems.append('data URLs in srcset are unsupported')
                else:
                    for candidate in value.split(','):
                        parts = candidate.split()
                        if parts:
                            self.media.add(parts[0])
        if tag == 'object' and attrs.get('data'):
            self.media.add(attrs['data'])
        if tag == 'link' and attrs.get('href'):
            rel = set(attrs.get('rel', '').lower().split())
            if 'canonical' in rel:
                self.canonicals.add(attrs['href'])
            if 'stylesheet' in rel:
                self.stylesheets.add(attrs['href'])
            if 'icon' in rel or ('preload' in rel and attrs.get('as') in {'image', 'video', 'audio', 'font'}):
                self.media.add(attrs['href'])
        if tag == 'script':
            self.scripts.append(attrs.get('src'))
        if attrs.get('style'):
            self.add_css(attrs['style'])
        if tag == 'style':
            self.in_style = True

    def handle_endtag(self, tag):
        if tag == 'style':
            self.in_style = False

    def handle_data(self, data):
        if self.in_style:
            self.add_css(data)

    def add_css(self, text):
        try:
            self.media.update(css_media(text))
        except ValueError as error:
            self.problems.append(str(error))


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate JSON key: {key}')
        result[key] = value
    return result


def local_url(value, base, origin):
    if not isinstance(value, str) or not value or value != value.strip():
        raise ValueError('URL must be a nonempty string without surrounding whitespace')
    decoded = unquote(value)
    if any(ord(char) <= 32 or ord(char) == 127 for char in value + decoded):
        raise ValueError('URL controls and spaces require a dedicated verifier')
    # Parse before decoding: %3F and %23 are path bytes, not URL delimiters.
    # Encoded separators have server-dependent handling, so reject them.
    raw_path = urlsplit(value).path
    if ('\\' in decoded or '..' in PurePosixPath(unquote(raw_path)).parts
            or re.search(r'%(?:2f|5c)', raw_path, re.IGNORECASE)):
        raise ValueError(f'URL traversal or encoded separator is unsupported: {value}')
    target = urlsplit(urljoin(origin + base, value))
    site = urlsplit(origin)
    if target.scheme not in {'http', 'https'} or target.netloc != site.netloc:
        raise ValueError(f'nonlocal URL requires a dedicated verifier: {value}')
    return unquote(target.path) or '/'


def check(root, contract_path):
    root = Path(root)
    hashes, contents, findings = {}, {}, []
    consumed_bytes = 0

    def finding(code, path, detail):
        findings.append({'code': code, 'path': path, 'detail': detail})

    def read(name):
        nonlocal consumed_bytes
        if not isinstance(name, str) or not name or '\\' in name or unquote(name) != name:
            raise ValueError('artifact path must be a nonempty, unencoded relative POSIX path')
        parts = PurePosixPath(name)
        if parts.is_absolute() or '..' in parts.parts:
            raise ValueError(f'artifact path escapes root: {name}')
        path = (root / name).resolve(strict=True)
        path.relative_to(root)
        if not path.is_file():
            raise ValueError(f'artifact is not a regular file: {name}')
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

    def load(name):
        return json.loads(read(name), object_pairs_hook=unique_object)

    def check_scripts(markup, path, base, policy):
        if not markup.scripts:
            return
        if policy != 'pinned_uninterpreted':
            finding('unsupported_dynamic_content', path, 'JavaScript is not analyzed; use a dedicated verifier or explicitly accept pinned uninterpreted scripts')
            return
        if path not in assets:
            raise ValueError('pinned uninterpreted scripts require a pinned page receipt, including inline code')
        for script in markup.scripts:
            if script:
                name = local_url(script, base, origin).lstrip('/')
                if name not in assets:
                    raise ValueError('every uninterpreted script requires a pinned receipt')
                read(name)

    try:
        root = root.resolve(strict=True)
        if not root.is_dir():
            raise ValueError('artifact root must be a directory')
        contract_name = Path(contract_path).resolve(strict=True).relative_to(root).as_posix()
        contract = load(contract_name)
        if contract.get('version') != 1:
            raise ValueError('unsupported route contract version')
        origin = contract.get('origin', 'https://fixture.invalid')
        parsed_origin = urlsplit(origin)
        if parsed_origin.scheme not in {'http', 'https'} or not parsed_origin.netloc or parsed_origin.path or parsed_origin.query or parsed_origin.fragment:
            raise ValueError('origin must be an HTTP(S) origin without a path')
        routes = contract['routes']
        if not isinstance(routes, list) or not 1 <= len(routes) <= 16:
            raise ValueError('contract must contain 1 to 16 routes')
        manifest, assets = load(contract['manifest']), contract['asset_sha256']
        if not isinstance(assets, dict) or not assets or len(assets) > 32:
            raise ValueError('contract requires 1 to 32 pinned SHA-256 receipts')
        reserved = set()
        for route in routes:
            canonical, aliases = route['canonical'], route.get('aliases', [])
            if not isinstance(aliases, list) or len(aliases) > 16:
                raise ValueError('route aliases exceed limit')
            for url in [canonical, *aliases]:
                if not isinstance(url, str) or not url.startswith('/') or url.startswith('//') or '?' in url or '#' in url or local_url(url, '/', origin) != url:
                    raise ValueError('route URLs must be absolute local paths without query, fragment or traversal')
                if url in reserved:
                    raise ValueError(f'duplicate canonical route or alias: {url}')
                reserved.add(url)
            actual = manifest.get(canonical, {})
            if actual.get('page') != route['page']:
                finding('wrong_page_mapping', canonical, f'expected page {route["page"]}')
            expected_media = route['media']
            if not isinstance(expected_media, list) or not 1 <= len(expected_media) <= 32 or len(set(expected_media)) != len(expected_media):
                raise ValueError('route requires 1 to 32 distinct media paths')
            if actual.get('media') != expected_media:
                finding('wrong_media_mapping', canonical, 'manifest media differs from contract')
            page = Markup(read(route['page']).decode('utf-8'))
            alias_modes = route.get('alias_modes', {})
            if not isinstance(alias_modes, dict) or any(key not in aliases for key in alias_modes):
                raise ValueError('alias_modes must name only declared aliases')
            for alias in aliases:
                mode = alias_modes.get(alias, 'redirect')
                expected = {'redirect': canonical}
                if mode == 'same_page':
                    expected = {'page': route['page'], 'canonical': canonical}
                    if {local_url(url, canonical, origin) for url in page.canonicals} != {canonical}:
                        finding('invalid_alias', alias, 'same-page alias requires the declared HTML canonical')
                elif mode != 'redirect':
                    raise ValueError('alias mode must be redirect or same_page')
                if manifest.get(alias) != expected:
                    finding('invalid_alias', alias, f'expected {mode} mapping to {canonical}')
            observed = {local_url(url, canonical, origin).lstrip('/') for url in page.media}
            for stylesheet in page.stylesheets:
                name = local_url(stylesheet, canonical, origin).lstrip('/')
                if name not in assets:
                    raise ValueError('every stylesheet requires a pinned receipt')
                observed.update(local_url(url, '/' + name, origin).lstrip('/') for url in css_media(read(name).decode('utf-8')))
            for problem in page.problems:
                finding('unsupported_static_content', route['page'], problem)
            check_scripts(page, route['page'], canonical, route.get('script_policy', 'reject'))
            for name in expected_media:
                if name not in assets:
                    raise ValueError('every route media file requires a pinned SHA-256 receipt')
                read(name)
            if set(expected_media) - observed:
                finding('wrong_page_media', route['page'], f'missing media: {sorted(set(expected_media) - observed)}')
            if observed - set(expected_media):
                finding('unexpected_page_media', route['page'], f'undeclared media: {sorted(observed - set(expected_media))}')
            entrypoints = route['entrypoints']
            if not isinstance(entrypoints, list) or not 1 <= len(entrypoints) <= 16:
                raise ValueError('route requires 1 to 16 entrypoints')
            for entry in entrypoints:
                allowed = entry.get('allowed_hrefs', [canonical])
                if not isinstance(allowed, list) or not allowed or any(url not in [canonical, *aliases] for url in allowed):
                    raise ValueError('entrypoint hrefs must name the canonical route or a declared alias')
                ignored = entry.get('ignore_hrefs', [])
                if not isinstance(ignored, list) or any(not isinstance(url, str) for url in ignored):
                    raise ValueError('ignored hrefs must be an explicit list of unrelated normalized local paths')
                markup = Markup(read(entry['path']).decode('utf-8'))
                base = entry.get('url', '/' + entry['path'])
                for problem in markup.problems:
                    finding('unsupported_static_content', entry['path'], problem)
                check_scripts(markup, entry['path'], base, entry.get('script_policy', 'reject'))
                links = {local_url(url, base, origin) for url in markup.links if not url.startswith('#')}
                if not links.intersection(allowed) or links - set(allowed) - set(ignored):
                    finding('stale_entry_link', entry['path'], f'expected only approved route links {allowed} and explicitly ignored unrelated links {ignored}; observed {sorted(links)}')
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
            'coverage': 'Declared route-page HTML media/srcset and CSS url(), plus explicit entry-page anchor links only. Scripts in either page type are rejected unless explicitly pinned and uninterpreted. Artifact hash is separate from Aether source_hash. No arbitrary JavaScript, imported dependency closure, browser, CDN, deployed-byte or runtime freshness assertion.'}


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
