#!/usr/bin/env python3
"""Isolated native-protocol provider: record the actual resumed CLI invocation."""
import json
import os
import sys

if '--version' in sys.argv:
    print('2.1.228')
    sys.exit(0)
if 'auth' in sys.argv:
    print(json.dumps({'loggedIn': True, 'authMethod': 'oauth', 'email': 'history@example.test'}))
    sys.exit(0)

for line in sys.stdin:
    message = json.loads(line)
    if message.get('type') == 'control_request':
        print(json.dumps({'type': 'control_response', 'response': {
            'subtype': 'success', 'request_id': message['request_id'],
            'response': {'models': [{'value': 'claude-sonnet-4-6', 'displayName': 'Sonnet 4.6'}], 'commands': []},
        }}), flush=True)
    elif message.get('type') == 'user':
        title_only = '--system-prompt' in sys.argv
        if not title_only:
            resume = next((arg.split('=', 1)[1] for arg in sys.argv if arg.startswith('--resume=')), None)
            assert resume == 'original-claude-session', 'Import continuation must resume the native session'
            with open(os.environ['ZUSE_IMPORT_PROVIDER_SEEN'], 'a') as log:
                log.write(json.dumps({'argv': sys.argv[1:], 'cwd': os.getcwd(), 'message': message}) + '\n')
        reply = 'Continued the original provider conversation.'
        print(json.dumps({'type': 'stream_event', 'event': {'type': 'content_block_delta',
            'delta': {'type': 'text_delta', 'text': reply}}}), flush=True)
        print(json.dumps({'type': 'result', 'subtype': 'success', 'result': reply,
            'usage': {'input_tokens': 1, 'output_tokens': 1}, 'session_id': 'original-claude-session'}), flush=True)
        break
