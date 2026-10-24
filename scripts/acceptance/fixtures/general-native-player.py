#!/usr/bin/env python3
"""Record native sound/open actions inside the disposable acceptance profile."""
import hashlib
import json
import os
from pathlib import Path
import sys
import wave

root = Path(os.environ['ZUSE_GENERAL_PROBE'])
if Path(sys.argv[0]).name == 'xdg-open':
    assert sys.argv[1] == 'https://identity.example.test/device'
    with (root / 'opened-urls.jsonl').open('a') as log:
        log.write(json.dumps({'url': sys.argv[1]}) + '\n')
else:
    source = Path(sys.argv[-1])
    data = source.read_bytes()
    assert data[:4] == b'RIFF' and data[8:12] == b'WAVE'
    with wave.open(str(source), 'rb') as audio:
        result = {'sha256': hashlib.sha256(data).hexdigest(),
                  'frames': audio.getnframes(), 'rate': audio.getframerate()}
    assert result['frames'] > 0 and result['rate'] > 0
    with (root / 'played-sounds.jsonl').open('a') as log:
        log.write(json.dumps(result) + '\n')
