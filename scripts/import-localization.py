#!/usr/bin/env python3
"""One-time, lossless catalog import. Runtime catalogs are edited as Fluent thereafter.

Usage: python3 scripts/import-localization.py /path/to/downloaded/desktop/catalogs
Input: <source>/<locale>/<namespace>.json at the revisions recorded below.
"""
import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1] / 'crates/i18n'
LOCALES = ['en', 'fr', 'de', 'zh-Hans', 'zh-Hant', 'ja', 'ko']
SOURCE = 'a0155d2dd11f07495a99d3737c9bd40a5c8b4508'
CHINESE = '0526c0854b75501b5d7dc7d917a06fd2c7be4231'
PLACEHOLDER = re.compile(r'{{\s*([\w]+)\s*}}')


def pattern(text):
    # Encode literal braces and whitespace as Fluent string expressions. In particular,
    # do not strip leading/trailing spaces from the contributor's sentence fragments.
    parts, cursor = [], 0
    for match in PLACEHOLDER.finditer(text):
        parts.append(literal(text[cursor:match.start()]))
        parts.append('{ $' + match[1] + ' }')
        cursor = match.end()
    parts.append(literal(text[cursor:]))
    return ''.join(parts) or '{ "" }'


def literal(text):
    # Most messages remain ordinary, translator-friendly Fluent text. Escape only
    # syntax characters, control characters and significant boundary whitespace.
    def escape(match):
        value = match[0].replace('\\', '\\u005C').replace('"', '\\u0022')
        value = value.replace('\n', '\\u000A').replace('\r', '\\u000D').replace('\t', '\\u0009')
        return '{ "' + value + '" }'
    return re.sub(r'^[ \t]+|[ \t]+$|[{}\n\r\t]', escape, text)


def main(source):
    if any((ROOT / 'locales').glob('*/*.ftl')):
        raise SystemExit('Refusing to overwrite translator-owned Fluent files. Use --output with an empty directory to audit the import.')
    ROOT.mkdir(parents=True, exist_ok=True)
    report = {'sourceRepository': 'swarajbachu/zuse', 'sourceRevision': SOURCE,
              'chineseRepository': 'liuweifeng/zuse', 'chineseRevision': CHINESE,
              'chinesePullRequest': 'https://github.com/swarajbachu/zuse/pull/952',
              'contributors': ['liuweifeng (Chinese native-speaker corrections)', 'Original Zuse translation contributors'],
              'policy': 'Preserve original wording. Imported translations remain draft; native screen QA is pending.',
              'messages': {}, 'exceptions': []}
    english = {}
    for file in sorted((source / 'en').glob('*.json')):
        ns = file.stem
        english[ns] = json.loads(file.read_text())
        for key, value in english[ns].items():
            report['messages'][f'{ns}:{key}'] = {
                'target': f'{ns}-{key}', 'english': value,
                'variables': sorted(set(PLACEHOLDER.findall(value))),
                'richParts': sorted(re.findall(r'</?(part\d+)>', value)),
                'sourceHash': hashlib.sha256(value.encode()).hexdigest(), 'translations': {}}
    for locale in LOCALES:
        dest = ROOT / 'locales' / locale
        dest.mkdir(parents=True, exist_ok=True)
        for ns, messages in english.items():
            translations = json.loads((source / locale / f'{ns}.json').read_text())
            lines = ['# Imported from Zuse desktop catalogs. See migration.json for provenance.',
                     '# Edit individual translations here; do not regenerate this file.', '']
            for key, value in translations.items():
                if key not in messages:
                    report['exceptions'].append({'locale': locale, 'key': f'{ns}:{key}', 'target': f'{ns}-{key}', 'textHash': hashlib.sha256(value.encode()).hexdigest(), 'reason': 'Locale-specific plural or source key; retained in catalog.'})
                else:
                    report['messages'][f'{ns}:{key}']['translations'][locale] = hashlib.sha256(value.encode()).hexdigest()
                if locale == 'en':
                    lines.append('# Source key: ' + ns + ':' + key)
                    if PLACEHOLDER.search(value):
                        lines.append('# Variables: ' + ', '.join(sorted(set(PLACEHOLDER.findall(value)))))
                lines.extend([f'{ns}-{key} = {pattern(value)}', ''])
            # Keep original suffix keys intact for migration checks; also provide a
            # complete logical message with Fluent's locale-aware plural selection.
            groups = {}
            for key in translations:
                match = re.fullmatch(r'(.+)_(zero|one|two|few|many|other)', key)
                if match:
                    groups.setdefault(match[1], {})[match[2]] = translations[key]
            for base, variants in groups.items():
                if 'other' not in variants:
                    continue
                lines.extend(['# Complete plural message. $count is numeric; remaining variables retain their source names.',
                              f'{ns}-{base}-plural =', '    { $count ->'])
                for category, value in variants.items():
                    prefix = '*' if category == 'other' else ''
                    lines.append(f'       {prefix}[{category}] {pattern(value)}')
                lines.extend(['    }', ''])
            (dest / f'{ns}.ftl').write_text('\n'.join(lines))
    (ROOT / 'migration.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(f'Imported {len(report["messages"])} source messages across {len(LOCALES)} locales.')


if __name__ == '__main__':
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('--output', type=Path, help='Empty output directory; existing catalogs are never overwritten')
    args = parser.parse_args()
    if args.output:
        ROOT = args.output
    main(args.source)
