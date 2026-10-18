#!/usr/bin/env python3
"""Deterministic native ACP peer: no network, tools, or external credentials."""
import json
import os
import sys
import time

marker = sys.argv[1]
assert sys.argv[2] == 'literal ; $(not-a-shell)'
assert os.environ.get('ACP_TEST_MARKER') == marker
session = f'session-{marker}'

def emit(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get('method')
    ident = frame.get('id')
    if method == 'initialize':
        emit({'id': ident, 'result': {'protocolVersion': 1, 'agentCapabilities': {'loadSession': True}}})
    elif method in ('session/new', 'session/load'):
        emit({'id': ident, 'result': {'sessionId': session, 'models': {
            'currentModelId': f'{marker}/model', 'availableModels': [
                {'modelId': f'{marker}/model', 'name': f'{marker} model'}]}}})
    elif method == 'session/prompt':
        prompt = '\n'.join(part.get('text', '') for part in frame['params']['prompt'])
        emit({'method': 'session/update', 'params': {'sessionId': session, 'update': {
            'sessionUpdate': 'agent_message_chunk', 'content': {'type': 'text', 'text': f'{marker}:{prompt}'}}}})
        if 'first' in prompt:
            time.sleep(0.3)
        emit({'id': ident, 'result': {'stopReason': 'end_turn'}})
    elif ident is not None:
        emit({'id': ident, 'result': {}})
