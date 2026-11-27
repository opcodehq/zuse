// Real coordinator/worker turns: a person's answer reaches exactly the thread
// it answers, once, and that thread's reply nests under the answer.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {execFileSync} from 'node:child_process';
import {engine, fixture, artifact, sleep} from './harness.mjs';
const target = process.env.CARGO_TARGET_DIR ?? execFileSync('python3', ['scripts/cargo_dev.py', '--print-target-dir'], {encoding: 'utf8'}).trim();
const binary = path.join(target, 'debug/zuse-cli');
const keep = process.env.ZAPPER_CONVERSATION_PROFILE;
let host;
try {
  host = await engine('zapper-conversation', {binary, ...(keep ? {profileRoot: keep} : {})});
  const info = await host.call('EngineInfo');
  const repo = path.join(host.root, 'repo'), trees = path.join(host.root, 'worktrees'), proof = path.join(host.root, 'proof.json');
  fs.mkdirSync(repo); fs.mkdirSync(trees);
  execFileSync('git', ['init', '-b', 'main'], {cwd: repo, stdio: 'ignore'});
  execFileSync('git', ['-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '--allow-empty', '-m', 'Initial'], {cwd: repo, stdio: 'ignore'});
  await host.call('SetWorktreeSettings', {useCustomDirectory: true, customDirectory: trees});
  await host.call('Mutate', {op: 'createSpace', spaceId: 'repo', deviceId: info.deviceId, path: repo, name: 'Web client', gitDetected: true});
  await host.call('SetCustomAcpAgent', {expectedRevision: 0, agent: {id: 'acp-zapper-conversation', name: 'Conversation fixture', command: 'node', args: [fixture('zapper-conversation-acp.mjs'), proof], env: {}, modelIds: []}});
  const config = {name: 'Release', purpose: 'Ship the Solid web client.', homeDeviceId: info.deviceId, repositories: [{deviceId: info.deviceId, spaceId: 'repo'}], coordinatorModel: {harness: 'acp-zapper-conversation', model: 'fixture/model'}, workerDeviceId: info.deviceId, maxConcurrentWorkers: 1, maxFollowups: 3};
  const created = await host.call('ZapperCommand', {commandId: 'create', zapperId: 'release', expectedRevision: null, action: {type: 'create', config}});
  await host.call('QueueMessage', {chatId: created.coordinatorChatId, text: 'File and deliver the deploy.', attachments: []});
  const worker = 'zapper-worker-fixture-delegate';
  async function until(predicate, label) {
    for (let n = 0; n < 900; n++) {
      const current = (await host.call('WatchZappers')).find(z => z.id === 'release');
      const report = fs.existsSync(proof) ? JSON.parse(fs.readFileSync(proof)) : {};
      if (report.fixtureError) throw new Error(report.fixtureError);
      const issue = current?.issues.find(i => i.id === 'deploy');
      if (issue && predicate(issue, report, current)) return {issue, report, current};
      await sleep(100);
    }
    throw new Error('deadline: ' + label);
  }
  const userMessages = async () => (await host.call('WatchDocMessages', {chatId: worker})).reset.filter(m => m.role === 'user');

  // 1. The coordinator asks the person a question for its thread.
  const asked = await until((i, p) => i.details.status === 'needsYou' && p.asked, 'question asked');
  const question = asked.issue.activity.find(a => a.kind === 'question');
  assert.equal(question.thread, worker, 'the question is tied to the thread that held the issue');
  assert.equal(asked.report.workerNoteRouted, false, 'a worker-scoped comment never routes');

  // 2. The person answers at the revision they read; a lost reply is retried with the same id.
  const answer = {commandId: 'answer-1', zapperId: 'release', expectedRevision: asked.current.revision, action: {type: 'commentIssue', issueId: 'deploy', text: 'yeah basically b'}};
  const first = await host.call('ZapperCommand', answer);
  const retried = await host.call('ZapperCommand', answer);
  assert.deepEqual(retried, first, 'a retry returns the original receipt');
  const answered = await until(i => i.activity.find(a => a.id === 'answer-1')?.delivery?.state === 'answered', 'first answer answered');
  const comment = answered.issue.activity.find(a => a.id === 'answer-1');
  assert.equal(comment.delivery.chatId, worker);
  const reply = answered.issue.activity.find(a => a.id === comment.delivery.reply);
  assert.equal(reply.replyTo, 'answer-1', 'the reply nests under the answer it answers');
  assert.equal(reply.actor, worker);
  assert.match(reply.text, /B it is/);
  assert.deepEqual(answered.report.answers, [{issue: 'ZAP-1', text: 'yeah basically b'}], 'the worker received the answer exactly once');
  assert.equal((await userMessages()).filter(m => m.id === 'answer-1:answer').length, 1);
  assert.deepEqual(answered.issue.details.owner, {type: 'thread', chatId: worker}, 'the answered thread holds the issue again');

  // 3. An explicit reply to the thread's message reaches that thread, and its reply joins the card.
  const followUp = {commandId: 'answer-2', zapperId: 'release', expectedRevision: answered.current.revision, action: {type: 'commentIssue', issueId: 'deploy', text: 'Ship the deploy PR first', replyTo: reply.id}};
  await host.call('ZapperCommand', followUp);
  const second = await until(i => i.activity.find(a => a.id === 'answer-2')?.delivery?.state === 'answered', 'second answer answered');
  const secondReply = second.issue.activity.find(a => a.id === second.issue.activity.find(x => x.id === 'answer-2').delivery.reply);
  assert.equal(secondReply.replyTo, 'answer-1', 'replies stay one level deep in the same card');
  assert.match(secondReply.text, /Will do: Ship the deploy PR first/);

  // 4. A restart replays nothing: same receipt, no new worker turn.
  await host.close();
  host = await engine('zapper-conversation', {binary, profileRoot: host.root});
  assert.deepEqual(await host.call('ZapperCommand', answer), first);
  await sleep(3000);
  const after = await until(() => true, 'after restart');
  assert.equal(after.report.answers.length, 2, 'no answer was replayed after restart');
  assert.equal(after.report.workerTurns, 3, 'one kickoff plus one turn per answer');
  assert.equal((await userMessages()).filter(m => m.id.endsWith(':answer')).length, 2);
  // The Zapper hears about each answer, told it was delivered and not to relay it.
  const events = await host.call('ZapperEvents', {zapperId: 'release'});
  for (const id of ['issue-answer-1', 'issue-answer-2'])
    assert.match(events.find(e => e.id === id)?.summary ?? '', /Delivered directly to thread zapper-worker-fixture-delegate[\s\S]*Don't relay/, id);
  artifact('zapper-issue-conversation-acceptance.json', {realEngine: true, paidModelCalls: 0, exactRecipient: worker, answersDelivered: after.report.answers.length, workerTurns: after.report.workerTurns, nestedReplies: 2, retryDuplicates: 0, restartReplays: 0, workerImpersonationRouted: false, profile: host.root});
  console.log('Zapper issue conversation accepted.');
} catch (error) { console.error('Zapper issue conversation failed: ' + error.message); process.exitCode = 1; } finally { await host?.close(); }
