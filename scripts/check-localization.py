#!/usr/bin/env python3
"""Track per-message translation review without rewriting translator-owned catalogs.

Run with --update after English changes to mark affected reviews needs-review.
Fluent syntax, references, variables, and migration fidelity are checked by Rust tests.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / 'crates/i18n'
ENTRY = re.compile(r'^(-?[a-zA-Z][\w-]*)\s*=\s*(.*(?:\n[ \t]+[^\n]*)*)', re.MULTILINE)


def hashes(locale="en"):
    result = {}
    for file in sorted((ROOT / 'locales' / locale).glob('*.ftl')):
        for key, pattern in ENTRY.findall(file.read_text()):
            if key in result:
                raise ValueError(f'Duplicate catalog key: {key}')
            result[key] = hashlib.sha256(pattern.encode()).hexdigest()
    # A shared term or referenced message can change the meaning of its users.
    # Include transitive dependencies so those reviews become stale as well.
    patterns = {}
    for file in sorted((ROOT / 'locales' / locale).glob('*.ftl')):
        patterns.update(ENTRY.findall(file.read_text()))
    def fingerprint(key, stack):
        if key in stack:
            raise ValueError(f'Cyclic Fluent reference: {key}')
        references = sorted(set(re.findall(r'\{\s*(-?[A-Za-z][\w-]*)', patterns[key])) & patterns.keys())
        dependencies = ''.join(fingerprint(ref, stack | {key}) for ref in references)
        return hashlib.sha256((patterns[key] + dependencies).encode()).hexdigest()
    return {key: fingerprint(key, set()) for key in patterns}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--update', action='store_true')
    args = parser.parse_args()
    path = ROOT / 'review.json'
    state = json.loads(path.read_text()) if path.exists() else {
        'defaultStatus': 'draft', 'sourceHashes': {}, 'reviews': {}}
    current = hashes()
    changed = {key for key, value in current.items() if state['sourceHashes'].get(key) != value}
    removed = set(state['sourceHashes']) - set(current)
    problems = []
    translations = {directory.name: hashes(directory.name) for directory in (ROOT / 'locales').iterdir() if directory.is_dir() and directory.name != 'en'}
    if args.update:
        # A changed source invalidates just its own translations, including drafts.
        for locale, catalog in translations.items():
            for key in changed & set(state['sourceHashes']) & set(catalog):
                review = state['reviews'].setdefault(locale, {}).setdefault(key, {})
                review['status'] = 'needs-review'
                review.setdefault('sourceHash', state['sourceHashes'][key])
    for locale, messages in state['reviews'].items():
        for key, review in messages.items():
            if review.get('status') not in ['draft', 'needs-review', 'reviewed']:
                problems.append(f'{locale}:{key}: invalid review status')
            if review['status'] == 'reviewed':
                if not review.get('reviewer') or not review.get('evidence'):
                    problems.append(f'{locale}:{key}: reviewed messages require reviewer and screen QA evidence')
                if review.get('sourceHash') != current.get(key) or review.get('translationHash') != translations.get(locale, {}).get(key):
                    if args.update:
                        review['status'] = 'needs-review'
                    else:
                        problems.append(f'{locale}:{key}: source or translation changed; run with --update')
    if args.update:
        state['sourceHashes'] = current
        path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + '\n')
    elif changed or removed:
        problems.append(f'{len(changed)} changed/new and {len(removed)} removed English messages; run with --update')
    if problems:
        raise SystemExit('\n'.join(problems))
    print(f'Localization review checked: {len(current)} messages; translations default to draft.')


if __name__ == '__main__':
    main()
