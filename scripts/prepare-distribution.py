#!/usr/bin/env python3
"""Fetch a verified existing release archive, or prepare native sources for a missing architecture."""
import argparse
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile

from distribution import check_archive, release_version, unpack


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--platform', required=True, choices=('linux', 'macos', 'windows'))
    parser.add_argument('--arch', required=True, choices=('x86_64', 'aarch64'))
    parser.add_argument('--cached', action='store_true', help='Reuse an input artifact downloaded from a previous run')
    parser.add_argument('--staged', action='store_true', help='Reuse an unpublished archive built earlier in this run')
    args = parser.parse_args()
    version = release_version(args.tag)
    incoming = Path('incoming')
    incoming.mkdir(exist_ok=True)
    suffix = '.zip' if args.platform == 'windows' else '.tar.gz'
    filename = f'zuse-{version}-{args.platform}-{args.arch}{suffix}'
    if args.staged:
        (incoming / 'SHA256SUMS').write_text(f'{hashlib.sha256((incoming / filename).read_bytes()).hexdigest()}  {filename}\n')
    if args.cached or args.staged:
        check_archive(incoming / filename, incoming / 'SHA256SUMS')
        with tempfile.TemporaryDirectory(prefix='zuse-cached-') as temporary:
            package, manifest = unpack(incoming / filename, Path(temporary))
            expected = (version, args.platform, args.arch)
            if (manifest['version'], manifest['platform'], manifest['arch']) != expected:
                raise ValueError('Cached archive does not match the requested release target')
            revision = subprocess.check_output(['gh', 'api', f'repos/opcodehq/zuse/commits/{args.tag}', '--jq', '.sha'], text=True).strip()
            provenance = manifest.get('provenance', {})
            if provenance.get('source_revision') != revision or provenance.get('working_tree_dirty'):
                raise ValueError('Cached archive was not built from the clean immutable release tag')
        found = True
    else:
        subprocess.run(['gh', 'release', 'download', args.tag, '-R', 'opcodehq/zuse', '--pattern', 'SHA256SUMS', '--dir', str(incoming)], check=True)
        # Listing first distinguishes an absent architecture from a failed download.
        listed = subprocess.check_output(['gh', 'release', 'view', args.tag, '-R', 'opcodehq/zuse', '--json', 'assets', '--jq', '.assets[].name'], text=True).splitlines()
        found = filename in listed
        if found:
            subprocess.run(['gh', 'release', 'download', args.tag, '-R', 'opcodehq/zuse', '--pattern', filename, '--dir', str(incoming)], check=True)
            check_archive(incoming / filename, incoming / 'SHA256SUMS')
        elif (args.platform, args.arch) not in (('macos', 'x86_64'), ('linux', 'aarch64')):
            raise ValueError('The original release archive is missing')
    with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
        output.write(f'found={str(found).lower()}\nfilename={filename}\nversion={version}\n')


if __name__ == '__main__':
    main()
