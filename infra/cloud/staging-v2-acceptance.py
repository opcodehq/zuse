#!/usr/bin/env python3
"""Operator-only staging v2 acceptance. Default validate is entirely offline.

Run on Hetzner beside staging-github-canary.py and staging-owner-device.py.
Never print owner credentials, grant frames, transcripts or terminal output.
Every mutating request is journaled BEFORE HTTP. Uncertain commands are inspected,
never blindly repeated. This tooling does not enable any rollout or capability.
"""
import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import hashlib
import stat
import time
import uuid
from urllib.parse import urlencode

API = 'https://api-rust-stag.zuse.sh'
MODE = 'server-admitted-v2'
STEPS = ('first', 'followup', 'pause', 'add', 'edit', 'remove', 'long-run',
         'steer-add', 'steer', 'urgent-add', 'send-now', 'stop', 'resume-queue',
         'stale-stop', 'after-stop')

class Failure(Exception):
    pass

def require(ok, code):
    if not ok:
        raise Failure(code)

def safe_id(value):
    return isinstance(value, str) and re.fullmatch(r'[A-Za-z0-9:_-]{1,200}', value)

def validate_config(config):
    require(config.get('apiOrigin') == API, 'staging_origin_required')
    for k in ('owner', 'projectId', 'connectionId', 'candidateReleaseId'):
        require(safe_id(config.get(k)), 'config_'+k+'_invalid')
    require(isinstance(config.get('model'), str) and re.fullmatch(r'[A-Za-z0-9._:/-]{1,160}', config['model']), 'model_invalid')
    require(config.get('scenario', 'standard') in ('standard', 'prelease-stop'), 'scenario_invalid')
    require(config.get('provider') in ('boxd', 'boat', 'e2b', 'daytona'), 'provider_invalid')
    require(config.get('agent') in ('codex', 'claude', 'grok'), 'agent_invalid')
    require(re.fullmatch(r'[0-9a-f]{64}', config.get('candidateSha256', '')), 'candidate_sha_required')
    require(re.fullmatch(r'[A-Za-z0-9._/-]{1,120}', config.get('baseRef', '')) and '..' not in config['baseRef'], 'base_ref_invalid')
    return {'offline': True, 'configValid': True, 'mode': MODE, 'steps': list(STEPS),
            'allocationAttempted': False, 'credentialsRead': False}

def prompt(run, label, *, long=False):
    path = run['prefix'] + '/' + label + '.log'
    return ("This is an isolated acceptance test on branch " + run['branch'] + ". "
            "Use a shell tool: mkdir -p " + run['prefix'] + "; printf '%s\\n' '" + label + "' >> " + path +
            "; test $(wc -l < " + path + ") -eq 1. " +
            ("Write a file named " + run['prefix'] + "/running containing ready, then run sleep 45 as a foreground shell tool. " if long else '') +
            "Make an ordinary file edit in " + run['prefix'] + "/edited.txt, verify it with a shell test, and run git status --short. "
            "Do not commit, push, install software, or access credentials. Reply with V2_ACCEPT_" + label.upper().replace('-', '_') + "_DONE.")

def input_for(text):
    return dict(text=text, attachments=[], fileRefs=[], skillRefs=[], annotations=[], asGoal=False)

def run_action(run, label, *, harness, long=False):
    mid = run['messageIds'][label]
    return {'type': 'submit', 'messageId': mid, 'attachments': [], 'payload': {
        'kind': 'run', 'messageId': mid, 'request': {'prompt': prompt(run, label, long=long),
        'cwd': '.', 'harness': harness, 'sandbox': 'danger-full-access', 'autoApprove': True,
        'model': run['model'], 'reasoning': None, 'resume': None, 'modelOptions': {}, 'attachments': []}}}

def new_run(config):
    uid = uuid.uuid4().hex
    return {'runId': uid, 'branch': 'zuse/v2-acceptance-'+uid[:12],
            'prefix': '.zuse-acceptance/'+uid, 'model': config['model'],
            'taskKey': str(uuid.uuid4()), 'messageIds': {k: str(uuid.uuid4()) for k in STEPS},
            'commandIds': {k: str(uuid.uuid4()) for k in STEPS},
            'queueIds': {k: str(uuid.uuid4()) for k in ('add', 'steer-add', 'urgent-add')}}

class Driver:
    def __init__(self, canary, config, directory, go=None):
        validate_config(config)
        self.c, self.cfg, self.root, self.go = canary, config, directory, go

    def read(self, name):
        return self.module.device.read_private(self.root/(name+'.json'))

    def write(self, name, value):
        self.module.device.write_private(self.root/(name+'.json'), value)

    def authorize(self, *, cleanup=False):
        require(self.go and self.go.get('approved') is True, 'explicit_root_go_required')
        for key in ('owner', 'candidateReleaseId', 'candidateSha256', 'provider'):
            require(self.go.get(key) == self.cfg[key], 'go_pin_mismatch')
        require(cleanup or self.go.get('expiresAt', 0) > time.time(), 'go_expired')
        require(self.go.get('runId') == self.read('run')['runId'], 'go_run_mismatch')

    def api(self, method, path, body=None):
        # Shared owner lock surrounds all calls, including refresh, in main().
        token = self.c.credential()
        status, value = self.c.flow.http.request(method, API+path, token=token, body=body)
        require(isinstance(value, dict), 'invalid_api_response')
        return status, value

    def get(self, path):
        code, value = self.api('GET', path)
        require(200 <= code < 300, 'read_http_'+str(code))
        return value

    def endpoint(self, suffix=''):
        w = self.read('create-result')['workspace']['workspaceId']
        return '/v1/cloud/workspaces/'+w+suffix

    def session(self):
        result = self.read('create-result')
        return result.get('initialSessionId') or result['workspace']['initialSessionId']

    def prepare(self):
        require(not self.read('run'), 'run_already_prepared')
        run = new_run(self.cfg)
        self.write('run', run)
        return {'prepared': True, 'runId': run['runId'], 'branch': run['branch'], 'allocationAttempted': False}

    def effect(self, name, path, body, expected=None, *, cleanup=False):
        self.authorize(cleanup=cleanup)
        # A journal, including a rejected request, is immutable. Reconcile instead
        # of assuming a lost response means the server did not execute anything.
        require(not self.read('intent-'+name), 'inspect_original_outcome_required')
        record = {'method': 'POST', 'path': path, 'body': body, 'startedAt': time.time()}
        self.write('intent-'+name, record)
        try:
            code, result = self.api('POST', path, body)
        except Exception:
            self.write('outcome-'+name, {'uncertain': True})
            raise Failure('request_outcome_uncertain_inspect_original') from None
        self.write('outcome-'+name, {'status': code, 'value': result, 'receivedAt': time.time(), 'durationMs': (time.time()-record['startedAt'])*1000})
        require(code == expected if expected else 200 <= code < 300, 'mutation_http_'+str(code))
        return result

    def create(self):
        self.node_preflight()
        context = self.get('/v1/cloud/context')
        require(context.get('ownerId') == self.cfg['owner'], 'owner_scope_changed')
        require(context.get('commandAdmissionVersion') == 2, 'v2_creation_not_enabled')
        caps = context.get('computerCapabilities', {})
        require(isinstance(caps, dict), 'provider_capabilities_missing')
        cap = next((p for p in caps.get('providers', []) if p.get('id') == self.cfg['provider']), None)
        require(cap and cap.get('commandAdmissionVersion') == 2, 'provider_v2_not_certified')
        run = self.read('run')
        body = dict(projectId=self.cfg['projectId'], providerId=self.cfg['provider'],
                    baseRef=self.cfg['baseRef'], branch=run['branch'], agent=self.cfg['agent'],
                    model=self.cfg['model'], agentConnectionId=self.cfg['connectionId'],
                    idempotencyKey=run['taskKey'], sizeId=self.cfg.get('sizeId', 'default'),
                    runtimeMode='full-access', initialMessageDelivery=MODE,
                    firstMessage=prompt(run, 'first'))
        result = self.effect('create', '/v1/cloud/workspaces', body)
        self.write('create-result', result)
        w = result['workspace']
        require(w.get('commandDeliveryMode') == MODE and w.get('commandAdmissionVersion') == 2, 'task_not_v2')
        return {'admitted': True, 'workspaceId': w['workspaceId'], 'mode': MODE}

    def queue(self):
        return self.get(self.endpoint('/task-commands/queue?'+urlencode({'sessionId': self.session()})))

    def queue_row(self, key):
        qid = self.read('run')['queueIds'][key]
        found = next((i for i in self.queue()['items'] if i['id'] == qid), None)
        require(found is not None, 'queue_row_missing')
        return found

    def step(self, name):
        run = self.read('run')
        sid = self.session()
        if name in ('followup', 'long-run', 'after-stop'):
            action = run_action(run, name, harness='claude-code' if self.cfg['agent'] == 'claude' else self.cfg['agent'], long=name == 'long-run')
        elif name in ('pause', 'resume-queue'):
            action = {'type': 'pause-queue', 'paused': name == 'pause'}
        elif name in ('add', 'steer-add', 'urgent-add'):
            require(self.queue()['paused'], 'pause_queue_before_queue_test')
            action = dict(type='queue', queueId=run['queueIds'][name], messageId=run['messageIds'][name],
                          input=input_for(prompt(run, name, long=name == 'urgent-add')), holdForTurnEnd=False)
        elif name in ('edit', 'remove', 'send-now', 'steer'):
            key = {'edit': 'add', 'remove': 'add', 'send-now': 'urgent-add', 'steer': 'steer-add'}[name]
            row = self.queue_row(key)
            kind = {'edit': 'edit', 'remove': 'delete', 'send-now': 'send-now', 'steer': 'steer-now'}[name]
            action = dict(type=kind, queueId=row['id'], revision=row['revision'])
            if name == 'edit': action['input'] = input_for(prompt(run, 'edited-queued'))
        elif name == 'stop':
            action = {'type': 'submit', 'messageId': run['messageIds'][name], 'payload': {'kind': 'interrupt'}, 'attachments': []}
        elif name == 'stale-stop':
            # Explicit stale turn must be obtained from a saved factual status;
            # never infer it from a user MID or a fabricated guest cursor.
            based = self.read('stale-turn')
            require(based and isinstance(based.get('turnId'), str), 'saved_factual_stale_turn_required')
            action = {'type': 'submit', 'messageId': run['messageIds'][name], 'payload': {'kind': 'interrupt'}, 'attachments': [], 'basedOn': based}
        else:
            raise Failure('unknown_step')
        body = dict(commandId=run['commandIds'][name], sessionId=sid, wake='preserve-sleep', action=action)
        result = self.effect(name, self.endpoint('/task-commands'), body, expected=409 if name == 'stale-stop' else None)
        return {'step': name, 'commandId': body['commandId'], 'durablyAdmitted': result.get('durablyAdmitted'), 'state': result.get('state')}

    def inspect(self, name):
        intent = self.read('intent-'+name)
        if name == 'first':
            require(self.read('intent-create'), 'initial_create_intent_required')
            cid = 'launch:'+self.read('create-result')['workspace']['workspaceId']
            return self.get(self.endpoint('/task-commands/'+cid))
        require(intent, 'original_intent_required')
        require(name != 'create', 'inspect_create_by_idempotency_and_database_before_retry')
        cid = intent['body']['commandId']
        status, result = self.api('GET', self.endpoint('/task-commands/'+cid))
        self.write('inspection-'+name, {'status': status, 'value': result})
        return {k: result.get(k) for k in ('commandId', 'executionCommandId', 'deliveryState', 'executionState', 'errorCode')}

    def history(self):
        result = self.get(self.endpoint('/sessions/'+self.session()+'/task-history'))
        self.write('history', result)
        return {'source': result.get('source'), 'admissionCount': len(result.get('commandAdmissions', [])),
                'paused': result.get('queuePaused'), 'historyReadDoesNotRequestWake': True}

    def node_preflight(self):
        node = Path(self.node_binary).resolve(strict=True)
        info = node.stat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and not info.st_mode & 0o022, 'trusted_operator_node_required')
        result = subprocess.run([str(node), '--version'], env={'PATH':'/usr/local/bin:/usr/bin:/bin'}, capture_output=True, text=True, timeout=5)
        require(result.returncode == 0 and re.fullmatch(r'v(2[2-9]|[3-9][0-9])\.[0-9]+\.[0-9]+\n?',result.stdout), 'node22_or_newer_required')
        check = subprocess.run([str(node), '-e', "process.exit(typeof WebSocket === 'function' ? 0 : 1)"], env={'PATH':'/usr/local/bin:/usr/bin:/bin'}, capture_output=True, timeout=5)
        require(check.returncode == 0, 'node_websocket_required')
        digest=hashlib.sha256()
        with node.open('rb') as f:
            for data in iter(lambda:f.read(1024*1024),b''):digest.update(data)
        return {'nodeVersion':result.stdout.strip(),'nodeSha256':digest.hexdigest(),'operatorBinary':str(node)}

    def preflight(self):
        node=self.node_preflight()
        context=self.get('/v1/cloud/context')
        require(context.get('ownerId')==self.cfg['owner'],'owner_scope_changed')
        return dict(node,metadataOnly=True,ownerMatches=True,commandAdmissionVersion=context.get('commandAdmissionVersion'),allocationAttempted=False)

    def gateway(self, action, observe_step='first'):
        self.authorize()
        self.node_preflight()
        # Never use effect() for a ticket: its credential must remain RAM-only.
        ticket_id = 'gateway-ticket-'+str(uuid.uuid4())
        self.write('intent-'+ticket_id, {'method': 'POST', 'path': self.endpoint('/gateway/ticket'),
                   'body': {'protocol': 'zuse-workspace-rust-v1', 'rpcProtocol': 'zuse-rpc-v1'}})
        code, grant = self.api('POST', self.endpoint('/gateway/ticket'),
                               {'protocol': 'zuse-workspace-rust-v1', 'rpcProtocol': 'zuse-rpc-v1'})
        self.write('outcome-'+ticket_id, {'status': code, 'credentialPersisted': False})
        require(200 <= code < 300, 'gateway_ticket_unavailable')
        run = self.read('run')
        payload = dict(grant=grant, sessionId=self.session(), prefix=run['prefix'], action=action,
                       expectedLabels=['after-stop'] if self.cfg.get('scenario') == 'prelease-stop' else ['first', 'followup', 'after-stop'],
                       forbiddenLabels=['first'] if self.cfg.get('scenario') == 'prelease-stop' else [],
                       expectedMarker='V2_ACCEPT_'+observe_step.upper().replace('-', '_')+'_DONE',
                       readyFile=str(self.root/('observer-ready-'+observe_step+'.json')))
        # Release the owner-session/refresh lock before waiting on a stream, so
        # another invocation may send the intended typed action concurrently.
        if getattr(self, 'operation_lock', None):
            fcntl.flock(self.operation_lock, fcntl.LOCK_UN)
        ready_path = self.root/('observer-ready-'+observe_step+'.json')
        if ready_path.exists() or ready_path.is_symlink():
            self.module.device.read_private(ready_path)  # verifies private owner/mode/no-follow
            ready_path.unlink()  # transient readiness only, never an action receipt
        probe = Path(__file__).with_name('staging-v2-gateway.mjs')
        child = subprocess.run([self.node_binary, str(probe)], input=json.dumps(payload), text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=190,
                               env={'PATH': '/usr/local/bin:/usr/bin:/bin'})
        require(len(child.stdout) <= 32768, 'gateway_report_too_large')
        result = json.loads(child.stdout)
        require(isinstance(result, dict) and result.get('metadataOnly') is True, 'invalid_gateway_report')
        self.write('gateway-'+action+'-'+str(time.time_ns()), result)
        if isinstance(result.get('lastCompletedTurn'), str) and not self.read('stale-turn'):
            self.write('stale-turn', {'turnId': result['lastCompletedTurn'], 'frontier': None})
        require(child.returncode == 0 and result.get('ok') is True, 'gateway_acceptance_failed')
        return result

    def cleanup_check(self):
        result = self.get(self.endpoint())
        workspace = result.get('workspace', result)
        require(workspace.get('state') == 'deleted', 'database_cleanup_not_finished')
        return {'workspaceId': workspace['workspaceId'], 'apiDeleted': True,
                'independentProviderAndDatabaseCheckStillRequired': True}

    def delete(self):
        wid = self.read('create-result')['workspace']['workspaceId']
        result = self.effect('delete', self.endpoint('/delete'), {'workspaceId': wid, 'commandId': str(uuid.uuid4())}, cleanup=True)
        return {'deleteRequested': True, 'workspaceId': wid, 'independentProviderAndDatabaseCheckStillRequired': True}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('action', choices=('validate', 'preflight', 'prepare', 'create', 'step', 'inspect', 'history', 'gateway', 'delete', 'cleanup-check'))
    p.add_argument('--config', type=Path, required=True)
    p.add_argument('--state-dir', type=Path)
    p.add_argument('--owner-dir', type=Path, default=Path('/etc/zuse/cloud/staging/owner-canary'))
    p.add_argument('--go-file', type=Path)
    p.add_argument('--step', choices=STEPS)
    p.add_argument('--node-binary', default='/var/tmp/zuse-harness-candidates.TIVvSo/node')
    p.add_argument('--observe-step', choices=STEPS, default='first')
    p.add_argument('--gateway-action', choices=('observe', 'verify'), default='observe')
    args = p.parse_args()
    # Offline validate never imports the authentication module or opens sessions.
    config = json.loads(args.config.read_text())
    if args.action == 'validate':
        print(json.dumps(validate_config(config), sort_keys=True)); return
    spec = importlib.util.spec_from_file_location('owner_canary', Path(__file__).with_name('staging-github-canary.py'))
    module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    require(args.state_dir is not None, 'private_state_directory_required')
    root = module.device.private_directory(args.state_dir)
    canary = module.Canary(args.owner_dir, config['owner'])
    driver = Driver(canary, config, root, module.device.read_private(args.go_file) if args.go_file else None)
    driver.module = module
    driver.node_binary = args.node_binary
    with (canary.root/'operation.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        driver.operation_lock = lock
        if args.action == 'step': result = driver.step(args.step)
        elif args.action == 'inspect': result = driver.inspect(args.step)
        elif args.action == 'gateway': result = driver.gateway(args.gateway_action, args.observe_step)
        elif args.action == 'cleanup-check': result = driver.cleanup_check()
        else: result = getattr(driver, args.action)()
    print(json.dumps(result, sort_keys=True))

if __name__ == '__main__':
    try: main()
    except Exception as error:
        print(json.dumps({'ok': False, 'code': str(error) if isinstance(error, Failure) else 'acceptance_'+type(error).__name__}))
        raise SystemExit(1)
