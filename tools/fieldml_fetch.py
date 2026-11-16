#!/usr/bin/env python3
"""Fetches the public blank forms the field classifier is trained and tested on.

The PDFs belong to their publishers and are not kept in the repository: being
offered for download does not grant a right to redistribute them. Each
manifest under crates/atelier-fieldml/data pins every file's URL and SHA-256,
so this script restores exactly the files the recorded runs used.

    python3 tools/fieldml_fetch.py           # fetch what is missing, check all
    python3 tools/fieldml_fetch.py --check   # check only; fetch nothing

A file whose download no longer matches its pinned hash is not kept: the
publisher has changed it, and the annotations made against the old revision
no longer describe it. Standard library only.
"""

import argparse
import hashlib
import json
import sys
import urllib.request
from pathlib import Path

DATA = Path(__file__).resolve().parents[1] / 'crates/atelier-fieldml/data'

# Manifest, folder the PDFs go in, and how an id becomes a file name.
MANIFESTS = [
    ('sources.json', 'sources', lambda id: id.removeprefix('public-')),
    ('applications.json', 'sources/applications', lambda id: id),
    ('v3/fresh-reviewed-sources.json', 'v3/sources', lambda id: id),
    ('v3/round3-sources.json', 'v3/sources', lambda id: id),
    ('v4/holdout-sources.json', 'v4/sources', lambda id: id),
    ('v5/holdout-sources.json', 'v5/sources', lambda id: id),
]

# A generic client name; nothing about who is fetching is sent.
AGENT = 'Mozilla/5.0 (compatible; atelier-fieldml-fetch/1)'


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def sources():
    """Every pinned source: (id, url, sha256, path)."""
    for manifest, folder, name in MANIFESTS:
        path = DATA / manifest
        if not path.exists():
            continue
        for s in json.loads(path.read_text()):
            if 'sha256' in s and 'url' in s:
                yield s['id'], s['url'], s['sha256'], DATA / folder / f"{name(s['id'])}.pdf"


def fetch(url):
    request = urllib.request.Request(url, headers={'User-Agent': AGENT})
    with urllib.request.urlopen(request, timeout=60) as response:
        return response.read()


def main():
    parser = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    parser.add_argument('--check', action='store_true', help='check only; fetch nothing')
    args = parser.parse_args()
    seen, problems = set(), 0
    for id, url, want, path in sources():
        if path in seen:
            continue
        seen.add(path)
        if path.exists() and sha256(path.read_bytes()) == want:
            print(f'ok       {id}')
            continue
        if args.check:
            print(f'{"changed" if path.exists() else "missing"}  {id}')
            problems += 1
            continue
        try:
            data = fetch(url)
        except Exception as e:  # noqa: BLE001 - reported per file
            print(f'failed   {id}: {e}')
            problems += 1
            continue
        if sha256(data) != want:
            print(f'changed  {id}: the download no longer matches its pinned hash; not kept')
            problems += 1
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        print(f'fetched  {id}')
    print(f'{len(seen)} sources, {problems} not available')
    return 1 if problems else 0


if __name__ == '__main__':
    sys.exit(main())
