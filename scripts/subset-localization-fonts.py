#!/usr/bin/env python3
"""Rebuild regional UI subsets from Noto Sans CJK 2.004 (requires fonttools).

Usage: python3 scripts/subset-localization-fonts.py /path/to/Noto-OTF-files
Keep the independent full CJK fallback for user text; subsets cover interface copy.
"""
from pathlib import Path
from copy import deepcopy
import hashlib
import json
import sys
from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.ttLib.scaleUpem import scale_upem
from fontTools.ttLib.tables._c_m_a_p import CmapSubtable
ROOT=Path(__file__).resolve().parents[1]
DEST=ROOT/'crates/viewport/assets/fonts'

def main(source):
    text=''.join(file.read_text() for file in (ROOT/'crates/i18n/locales').rglob('*.ftl'))
    characters=set(range(32,127))|{ord(c) for c in text}
    records={}
    for region in ['sc','tc','jp','kr']:
        original=source/f'NotoSansCJK{region}-Regular.otf'
        font=TTFont(original, recalcTimestamp=False)
        options=subset.Options()
        options.layout_features=['*']
        sub=subset.Subsetter(options=options)
        sub.populate(unicodes=characters)
        sub.subset(font)
        family=f'Zuse CJK {region.upper()}'
        for record in font['name'].names:
            if record.nameID in [1,4,6,16,21]:
                name=family.replace(' ','')+'-Regular' if record.nameID==6 else family
                record.string=name.encode(record.getEncoding(),errors='replace')
        output=DEST/f'ZuseCJK-{region}.otf'
        font.save(output)
        language=dict(sc='SimplifiedChinese',tc='TraditionalChinese',jp='Japanese',kr='Korean')[region]
        records[output.name]={'source':f'https://github.com/notofonts/noto-cjk/blob/Sans2.004/Sans/OTF/{language}/{original.name}', 'sourceSha256':hashlib.sha256(original.read_bytes()).hexdigest(),'sha256':hashlib.sha256(output.read_bytes()).hexdigest()}
    records['ZuseKorean-Fallback.otf'] = build_korean_fallback(source)
    records['ZuseCJK-Fallback.ttf'] = build_fallback(source)
    (DEST/'sources.json').write_text(json.dumps(records,indent=2)+'\n')

def build_korean_fallback(source):
    original=source/'NotoSansCJKkr-Regular.otf'
    font=TTFont(original, recalcTimestamp=False)
    # Retain every modern Hangul syllable, Jamo and halfwidth Korean character.
    characters=set(range(32,127)) | set(range(0x1100,0x1200)) | set(range(0x3130,0x3190)) | set(range(0xA960,0xA980)) | set(range(0xAC00,0xD800)) | set(range(0xFFA0,0xFFDD))
    sub=subset.Subsetter(options=subset.Options())
    sub.populate(unicodes=characters)
    sub.subset(font)
    for record in font['name'].names:
        if record.nameID in [1,4,6,16,21]:
            name='ZuseKoreanFallback-Regular' if record.nameID==6 else 'Zuse Korean Fallback'
            record.string=name.encode(record.getEncoding(),errors='replace')
    output=DEST/'ZuseKorean-Fallback.otf'
    font.save(output)
    return {'source':'https://github.com/notofonts/noto-cjk/blob/Sans2.004/Sans/OTF/Korean/NotoSansCJKkr-Regular.otf',
        'sourceSha256':hashlib.sha256(original.read_bytes()).hexdigest(), 'sha256':hashlib.sha256(output.read_bytes()).hexdigest()}

def build_fallback(source):
    # The text engine requires Latin metrics on every explicitly configured face.
    # Add the original Droid Sans Latin face to its full CJK companion, scaling
    # outlines to common units and retaining all encoded characters.
    latin_path=source/'DroidSans.ttf'
    cjk_path=source/'DroidSansFallbackFull.ttf'
    latin=TTFont(latin_path, recalcTimestamp=False)
    full=TTFont(cjk_path, recalcTimestamp=False)
    scale_upem(full,latin['head'].unitsPerEm)
    renames={name:'cjk_'+name for name in full.getGlyphOrder()}
    order=latin.getGlyphOrder()+list(renames.values())
    for name in full.getGlyphOrder():
        glyph=deepcopy(full['glyf'][name])
        glyph.removeHinting()
        if glyph.isComposite():
            for component in glyph.components:
                component.glyphName=renames[component.glyphName]
        latin['glyf'].glyphs[renames[name]]=glyph
        latin['hmtx'].metrics[renames[name]]=full['hmtx'].metrics[name]
    latin.setGlyphOrder(order)
    supplementary=CmapSubtable.newSubtable(12)
    supplementary.platformID=3
    supplementary.platEncID=10
    supplementary.language=0
    supplementary.cmap=dict(latin.getBestCmap())
    latin['cmap'].tables.append(supplementary)
    for table in latin['cmap'].tables:
        if table.isUnicode():
            for code,name in full.getBestCmap().items():
                if code<=0xFFFF or table.format==12:
                    table.cmap.setdefault(code,renames[name])
    for record in latin['name'].names:
        if record.nameID in [1,4,6,16,21]:
            name='ZuseCJKFallback-Regular' if record.nameID==6 else 'Zuse CJK Fallback'
            record.string=name.encode(record.getEncoding(),errors='replace')
    latin['OS/2'].recalcUnicodeRanges(latin)
    options=subset.Options()
    options.hinting=False
    sub=subset.Subsetter(options=options)
    sub.populate(unicodes=latin.getBestCmap())
    sub.subset(latin)
    path=DEST/'ZuseCJK-Fallback.ttf'
    latin.save(path)
    return {'source':'google-droid-sans-fonts-20200215-9.amzn2023.0.2 (Android Open Source Project; Apache-2.0)',
        'upstream':'https://android.googlesource.com/platform/frameworks/base/+/android-4.4_r1/data/fonts/',
        'inputs':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [latin_path,cjk_path]},
        'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}

if __name__=='__main__':main(Path(sys.argv[1]))
