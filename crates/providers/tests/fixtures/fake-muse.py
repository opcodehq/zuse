#!/usr/bin/env python3
"""Fake `muse serve`: the Muse Session Protocol subset Zuse's driver speaks.

Every inbound frame is appended to `commands.jsonl` in the working directory
so tests can assert on the wire. Behaviour switches:
  FAKE_MUSE_HANG=1      never answer initialize
  FAKE_MUSE_SCHEMA=<n>  report schema version n
  FAKE_MUSE_PROFILE=<p> report permission profile p
  FAKE_MUSE_NO_MCP=1    do not grant sessionMcp
The prompt text (displayText) selects a turn script: gap, crash, wait,
approve, question; anything else streams a reply.
"""
import json
import os
import sys
import threading
import time

if '--version' in sys.argv:
    print('muse 1.4.6')
    sys.exit(0)

lock = threading.RLock()
state = {'cursor': 0, 'session': 'native-muse-session', 'turn': None, 'running': False, 'gap': []}


def output(message):
    message['jsonrpc'] = '2.0'
    with lock:
        sys.stdout.write(json.dumps(message) + '\n')
        sys.stdout.flush()


def cursor():
    state['cursor'] += 1
    return str(state['cursor'])


def event(method, params):
    with lock:
        body = {'sessionId': state['session'], 'sourceRange': {'start': 0, 'end': 0}, 'viewCursor': cursor()}
        body.update(params)
        output({'method': method, 'params': body})


def finish(terminal='completed'):
    state['running'] = False
    event('turn/completed', {
        'turnId': state['turn'], 'terminal': terminal,
        'usage': {'inputTokens': 12, 'outputTokens': 3, 'cachedTokens': 2, 'reasoningTokens': 0},
    })


ITEM = {'itemId': 'message', 'kind': 'agentMessage', 'status': 'inProgress', 'revision': 1, 'text': ''}


def message(text='Hello 🌍', item_id='message'):
    item = dict(ITEM, itemId=item_id, turnId=state['turn'])
    event('item/started', {'item': item})
    event('item/delta', {'itemId': item_id, 'delta': text[:6]})
    event('item/delta', {'itemId': item_id, 'delta': text[6:]})
    event('session/contextUsage', {'usedTokens': 15, 'windowTokens': 1000, 'pressure': 'normal'})

    def later():
        time.sleep(0.05)
        event('item/completed', {'item': dict(item, text=text, revision=2, status='completed')})
        finish()
    threading.Thread(target=later, daemon=True).start()


def error(request, code, message, kind):
    output({'id': request['id'], 'error': {'code': code, 'message': message, 'data': {'kind': kind}}})


def turn_start(request, params):
    state['turn'] = params['commandId']
    state['running'] = True
    output({'id': request['id'], 'result': {
        'turnId': state['turn'], 'commandId': params['commandId'], 'status': 'accepted',
        'disposition': 'started', 'startedNewTurn': True}})
    event('turn/started', {'turnId': state['turn'], 'commandId': params['commandId']})
    text = params.get('displayText', '')
    if text == 'gap':
        after = str(state['cursor'])
        state['gap'] = []
        for method, body in [
            ('item/completed', {'item': dict(ITEM, turnId=state['turn'], revision=2, status='completed', text='Recovered reply')}),
            ('turn/completed', {'turnId': state['turn'], 'terminal': 'completed'}),
        ]:
            full = {'sessionId': state['session'], 'sourceRange': {'start': 0, 'end': 0}, 'viewCursor': cursor()}
            full.update(body)
            state['gap'].append({'method': method, 'params': full})
        state['running'] = False
        output({'method': 'view/gap', 'params': {'sessionId': state['session'], 'after': after, 'next': str(state['cursor'])}})
        return
    if text == 'crash':
        sys.stdout.flush()
        os._exit(7)
    if text == 'wait':
        return
    if text == 'approve':
        approval = {
            'approvalId': 'approval', 'itemId': 'tool', 'toolCallId': 'tool', 'taskId': 'tool',
            'currentRequirementId': {'approvalId': 'approval', 'sourceIndex': 0},
            'toolName': 'Bash', 'rawArgs': '{"command":"ls"}', 'protectedWrite': False,
            'judgeEscalated': False, 'subject': {'kind': 'shell', 'command': 'ls'},
            'turnId': state['turn'],
            'availableChoices': [
                {'choiceId': 'always', 'decision': 'approved', 'scope': 'localPersistent', 'label': 'Always'},
                {'choiceId': 'yes', 'decision': 'approved', 'scope': 'once', 'label': 'Allow'},
                {'choiceId': 'no', 'decision': 'denied', 'scope': 'once', 'label': 'Deny'},
            ],
        }
        event('item/started', {'item': {'itemId': 'tool', 'kind': 'toolCall', 'tool': 'Bash', 'args': '{"command":"ls"}', 'status': 'inProgress', 'revision': 1, 'turnId': state['turn']}})
        event('approval/requested', approval)
        return
    if text == 'question':
        event('userInput/requested', {
            'userInputId': 'question', 'itemId': 'question', 'toolCallId': 'question',
            'toolName': 'AskUserQuestion', 'turnId': state['turn'],
            'questions': [{'id': 'q', 'header': 'Pick', 'question': 'Which one?',
                           'options': [{'label': 'One'}, {'label': 'Two'}], 'selection': {'mode': 'single'}}],
        })
        return
    message()


for line in sys.stdin:
    request = json.loads(line)
    with open(os.path.join(os.getcwd(), 'commands.jsonl'), 'a') as log:
        log.write(json.dumps(request) + '\n')
    if 'id' not in request:
        continue
    params = request.get('params') or {}
    method = request.get('method')

    def reply(result):
        output({'id': request['id'], 'result': result})

    if method == 'initialize':
        if os.environ.get('FAKE_MUSE_HANG') == '1':
            continue
        reply({
            'serverInfo': {'name': 'fake-muse', 'version': '1.4.6'},
            'schema': {'version': int(os.environ.get('FAKE_MUSE_SCHEMA', '1')), 'fingerprint': 'sha256:' + '0' * 64},
            'grantedCapabilities': [] if os.environ.get('FAKE_MUSE_NO_MCP') == '1' else ['sessionMcp'],
            'sessionDurability': 'durable', 'experimentalApi': False, 'museHome': '/tmp',
            'platformFamily': 'unix', 'platformOs': 'linux', 'userAgent': 'fake',
            'environment': {'MUSE_NO_AUTO_UPDATE': os.environ.get('MUSE_NO_AUTO_UPDATE')},
        })
    elif method == 'model/list':
        reply({'providerId': 'meta', 'profileId': None, 'source': 'providerCatalog', 'models': [
            {'modelId': 'muse-test', 'displayLabel': 'Muse Test', 'providerId': 'meta', 'isDefault': True, 'variants': ['high', 'max']},
            {'modelId': 'other', 'displayLabel': 'Other', 'providerId': 'other', 'isDefault': False},
            {'modelId': 'muse-test', 'displayLabel': 'Duplicate', 'providerId': 'meta', 'isDefault': False},
            {'modelId': 'muse-lite', 'displayLabel': 'Muse Lite', 'providerId': 'meta', 'isDefault': False, 'variants': 'unknown'},
        ]})
    elif method == 'view/page':
        reply({'events': state['gap'], 'nextCursor': None})
    elif method in ('session/start', 'session/resume'):
        if params.get('sessionId') == 'missing':
            error(request, -32000, 'Session not found', 'sessionNotFound')
            continue
        state['session'] = params.get('sessionId', state['session'])
        reply({'session': {'sessionId': state['session'], 'modelId': 'muse-test',
                           'permissionState': {'profileId': os.environ.get('FAKE_MUSE_PROFILE', ':ask-me')}},
               'viewCursor': '0'})
    elif method in ('session/setApprovalMode', 'session/setModel', 'task/stop'):
        reply({'commandId': params.get('commandId'), 'status': 'accepted'})
    elif method == 'session/compact':
        state['turn'] = params['commandId']
        state['running'] = True
        reply({'commandId': params['commandId'], 'status': 'accepted'})
        event('turn/started', {'turnId': state['turn'], 'commandId': params['commandId']})
        event('item/completed', {'item': {'itemId': 'compaction', 'kind': 'compaction', 'status': 'completed', 'revision': 1, 'outcome': 'compacted'}})
        finish()
    elif method == 'turn/start':
        turn_start(request, params)
    elif method == 'turn/steer':
        if not state['running'] or params.get('expectedTurnId') != state['turn']:
            error(request, -32030, 'no running turn', 'commandRejected')
            continue
        reply({'commandId': params['commandId'], 'status': 'accepted', 'turnId': state['turn']})
        event('item/completed', {'item': {'itemId': 'steer', 'kind': 'userMessage', 'status': 'completed', 'revision': 1,
                                          'commandId': params['commandId'], 'steered': True, 'text': params['input'][0]['text']}})
        message('Steered reply', 'steered-message')
    elif method == 'approval/decide':
        reply({'commandId': params['commandId'], 'status': 'accepted', 'terminal': True, 'approvalId': params['approvalId']})
        decision = 'approved' if params['choiceId'] == 'yes' else 'denied'
        event('approval/resolved', {'approvalId': params['approvalId'], 'decision': decision})
        event('item/completed', {'item': {'itemId': 'tool', 'kind': 'toolCall', 'tool': 'Bash', 'args': '{"command":"ls"}',
                                          'status': 'completed' if decision == 'approved' else 'rejected', 'revision': 2,
                                          'visibleOutput': 'file.txt' if decision == 'approved' else 'denied by user'}})
        message()
    elif method == 'userInput/answer':
        reply({'commandId': params['commandId'], 'status': 'accepted', 'userInputId': params['userInputId']})
        event('userInput/settled', {'userInputId': params['userInputId'], 'outcome': 'answered', 'answers': params['answers']})
        message()
    elif method == 'userInput/cancel':
        reply({'commandId': params['commandId'], 'status': 'accepted', 'userInputId': params['userInputId']})
        finish('cancelled')
    elif method == 'turn/interrupt':
        reply({'commandId': params['commandId'], 'status': 'accepted', 'turnId': state['turn']})
        if state['running']:
            finish('cancelled')
    else:
        reply({})
