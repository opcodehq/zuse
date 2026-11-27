// A deterministic coordinator and worker for the issue conversation flow; no model API.
// The coordinator files and delegates one issue, then asks the person a question
// for its thread. The worker asks "A or B?", then answers each routed reply.
import fs from 'node:fs';
import readline from 'node:readline';
const [proofFile] = process.argv.slice(2);
let env = {};
const proof = () => fs.existsSync(proofFile) ? JSON.parse(fs.readFileSync(proofFile, 'utf8')) : {};
const record = patch => fs.writeFileSync(proofFile, JSON.stringify({...proof(), ...patch}));
async function control(method, params) {
  const ws = new WebSocket('ws://127.0.0.1:' + env.ZUSE_IPC_PORT);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => { ws.close(); reject(new Error('Scoped fixture RPC timed out')); }, 15000);
    ws.onmessage = e => { const frame = JSON.parse(e.data); if (frame.id !== 1) return; clearTimeout(timer); ws.close(); frame.err ? reject(new Error(String(frame.err))) : resolve(frame.ok); };
    ws.send(JSON.stringify({id: 1, method: 'ControlCall', params: {token: env.ZUSE_CONTROL_TOKEN, method, params}}));
  });
}
const command = (id, action) => control('ZapperCommand', {commandId: id, zapperId: env.ZUSE_ZAPPER_ID ?? env.ZUSE_ZAPPER_WORKER_ID, expectedRevision: null, action});
const say = text => process.stdout.write(JSON.stringify({jsonrpc: '2.0', method: 'session/update', params: {sessionId: 'conversation-fixture', update: {sessionUpdate: 'agent_message_chunk', content: {type: 'text', text}}}}) + '\n');
for await (const line of readline.createInterface({input: process.stdin})) {
  const frame = JSON.parse(line); let result = {};
  try {
    if (frame.method === 'initialize') result = {protocolVersion: 1, agentCapabilities: {loadSession: true}};
    else if (frame.method === 'session/new' || frame.method === 'session/load') {
      env = Object.fromEntries((frame.params.mcpServers?.find(s => s.name === 'zuse')?.env ?? []).map(e => [e.name, e.value]));
      result = {sessionId: 'conversation-fixture', models: {currentModelId: 'fixture/model', availableModels: [{modelId: 'fixture/model', name: 'Fixture'}]}};
    } else if (frame.method === 'session/prompt') {
      const text = (frame.params.prompt ?? []).map(p => p.text ?? '').join('\n');
      if (env.ZUSE_ZAPPER_ID) {
        const state = proof();
        if (!state.delegated) {
          const zapper = await command('fixture-create', {type: 'createIssue', issueId: 'deploy', details: {title: 'Deploy the Solid web client', description: 'Serve the Solid app at code.zuse.sh.', status: 'todo', priority: 'high', assigned: true, dueDate: null, labels: [], relations: [], doneCriteria: 'code.zuse.sh serves the app from main.'}});
          const repository = zapper.config.repositories[0];
          await command('fixture-delegate', {type: 'delegate', issueId: 'deploy', worker: {chatId: 'zapper-worker-fixture-delegate', deviceId: repository.deviceId, spaceId: repository.spaceId, objective: 'Deploy the Solid web client.', followups: 0}, model: zapper.config.coordinatorModel, base: 'main'});
          record({delegated: true});
        } else if (state.workerAsked && !state.asked) {
          await command('fixture-ask', {type: 'askUser', issueId: 'deploy', question: 'Should code.zuse.sh use the proxy (A) or tokens (B)?', recommendation: 'B'});
          record({asked: true});
        }
        record({coordinatorTurns: (state.coordinatorTurns ?? 0) + 1, coordinatorSawRelayWarning: (state.coordinatorSawRelayWarning ?? false) || text.includes("Don't relay this comment")});
        const events = await control('ZapperEvents', {zapperId: env.ZUSE_ZAPPER_ID});
        await control('AckZapperEvents', {zapperId: env.ZUSE_ZAPPER_ID, eventIds: events.map(e => e.id)});
        say('Coordinator update recorded.');
      } else if (env.ZUSE_ZAPPER_WORKER_ID) {
        const state = proof();
        const answer = text.match(/The person replied on (ZAP-\d+) "[^"]*":\n\n([^\n]*)/);
        if (!answer) {
          // A worker-scoped comment can never impersonate the person.
          const zapper = await command('fixture-worker-note', {type: 'commentIssue', issueId: 'deploy', text: 'Starting with the deploy.'});
          const issue = zapper.issues.find(i => i.id === 'deploy');
          const own = issue.activity.find(a => a.id === 'fixture-worker-note');
          record({workerAsked: true, workerTurns: 1, workerNoteRouted: !!own.delivery});
          say('Should code.zuse.sh use the proxy (A) or tokens (B)?');
        } else {
          const answers = [...(state.answers ?? []), {issue: answer[1], text: answer[2]}];
          record({answers, workerTurns: (state.workerTurns ?? 0) + 1});
          say(answers.length === 1 ? 'B it is. I will build the Cloudflare deploy first.' : 'Will do: ' + answer[2]);
        }
      } else throw new Error('Missing engine-stamped Zapper scope');
      result = {stopReason: 'end_turn'};
    }
    if (frame.id !== undefined) process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: frame.id, result}) + '\n');
  } catch (error) {
    record({fixtureError: error.message});
    if (frame.id !== undefined) process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: frame.id, error: {code: -32000, message: error.message}}) + '\n');
  }
}
