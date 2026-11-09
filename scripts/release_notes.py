#!/usr/bin/env python3
"""Select one changelog section, preferring the exact preview version."""
from pathlib import Path
import re
import sys

from distribution import release_version


def release_notes(changelog, tag):
    version = release_version(tag)
    headings = list(re.finditer(r'^##[ \t]+([^\r\n]+)\r?$', changelog, re.MULTILINE))
    for candidate in dict.fromkeys((version, version.split('-', 1)[0])):
        matches = [i for i, heading in enumerate(headings)
                   if heading[1].split(' - ', 1)[0].strip() == candidate]
        if len(matches) > 1:
            raise ValueError(f'Duplicate changelog section: {candidate}')
        if matches:
            index = matches[0]
            end = headings[index + 1].start() if index + 1 < len(headings) else len(changelog)
            notes = changelog[headings[index].end():end].strip()
            if not notes:
                raise ValueError(f'Empty changelog section: {candidate}')
            return notes + '\n'
    raise ValueError(f'No changelog section for {version}')


if __name__ == '__main__':
    try:
        print(release_notes(Path('CHANGELOG.md').read_text(), sys.argv[1]), end='')
    except (ValueError, IndexError) as error:
        raise SystemExit(str(error)) from error
