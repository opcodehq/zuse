// Two real engines, one browser sidebar, duplicate chat ids and isolated live workspaces.
import assert from 'node:assert/strict';
import { buildApp, openBrowser, startEngine } from './harness.mjs';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
const root = mkdtempSync(join(tmpdir(), 'zuse-multi-'));
const gitEnv = { ...process.env, GIT_AUTHOR_NAME: 'Fixture', GIT_AUTHOR_EMAIL: 'fixture@example.invalid', GIT_COMMITTER_NAME: 'Fixture', GIT_COMMITTER_EMAIL: 'fixture@example.invalid' };
const assets = process.env.ZUSE_ASSETS ?? buildApp();
let local, remote, page;
const git = (args, cwd) => execFileSync('git', args, { cwd, env: gitEnv, stdio: 'ignore' });
const wait = ms => new Promise(resolve => setTimeout(resolve, ms));
try {
  git(['init', '-q', '-b', 'main'], root);
  writeFileSync(join(root, 'README.md'), '# Shared project\n');
  git(['add', '.'], root); git(['commit', '-qm', 'Initial'], root);
  const localProject = join(root, 'local'); const remoteProject = join(root, 'remote');
  git(['clone', '-q', root, localProject], root); git(['clone', '-q', root, remoteProject], root);
  local = await startEngine({ assets }); remote = await startEngine({ assets });
  const config = { harness: 'claude-code', model: null, reasoning: null, sandbox: 'workspace-write' };
  for (const [engine, path, label] of [[local, localProject, 'Local login'], [remote, remoteProject, 'Remote deploy']]) {
    const info = await engine.rpc('EngineInfo');
    await engine.rpc('Mutate', { op: 'createSpace', spaceId: 'same-project-id', deviceId: info.deviceId, path, name: 'Shared project', gitDetected: true });
    await engine.rpc('Mutate', { op: 'createChat', chatId: 'same-chat-id', spaceId: 'same-project-id', config });
    await engine.rpc('Mutate', { op: 'renameChat', chatId: 'same-chat-id', title: label });
  }
  page = await openBrowser(local.url, { width: 1440, height: 950 });
  await page.waitFor(`window.__zuse?.state.workspaceSynced && document.querySelector('[data-chat-id="same-chat-id"] .sb-row-open')`);
  await page.click('[data-chat-id="same-chat-id"] .sb-row-open');
  await page.waitFor(`document.querySelector('.cmp-input')`);
  await page.press('.cmp-input'); await page.type('Keep the local draft');
  await page.click('button[aria-label="Settings"]');
  await page.waitFor(`[...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Remote access')`);
  await page.eval(`[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Remote access').click()`);
  await page.waitFor(`[...document.querySelectorAll('button')].some(button => button.textContent.trim() === 'Add computer')`);
  await page.eval(`[...document.querySelectorAll('button')].find(button => button.textContent.trim() === 'Add computer').click()`);
  const invitation = await remote.rpc('CreatePairing', { label: 'Browser multi-computer test' });
  const link = `${invitation.origin}/${invitation.fragment}`;
  await page.press('input[aria-label="Private connection link"]'); await page.type(link);
  await page.eval(`[...document.querySelectorAll('[role="dialog"] button')].find(button => button.textContent.trim() === 'Connect').click()`);
  await page.waitFor(`document.querySelectorAll('.mc-thread').length === 2`, 30000);
  assert.equal(await page.eval(`document.querySelectorAll('iframe.mc-frame').length`), 1);
  assert.deepEqual((await page.eval(`[...document.querySelectorAll('.mc-thread-title')].map(element => element.textContent)`)).sort(), ['Local login', 'Remote deploy']);
  await page.waitFor(`[...document.querySelectorAll('.mc-frame')].some(frame => frame.contentWindow.__zuse?.state.workspaceSynced)`, 30000);
  const remoteId = await page.eval(`document.querySelector('.mc-frame').contentWindow.__zuse.state.localDeviceId`);
  const localId = await page.eval(`window.__zuse.state.localDeviceId`);
  assert.notEqual(remoteId, localId);
  // Enter selects the scoped remote chat, even though its id equals the local chat's id.
  await page.eval(`[...document.querySelectorAll('.mc-thread')].find(button => button.textContent.includes('Remote deploy')).click()`);
  await page.waitFor(`document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input')`);
  await page.eval(`(() => { const input = document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input'); input.focus(); input.value = 'Keep the remote draft'; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await page.eval(`document.querySelector('.mc-thread[data-computer="primary"]').click()`);
  await page.waitFor(`document.querySelector('.mc-outlet:not([hidden]) .cmp-input')?.value === 'Keep the local draft'`);
  // A background update changes the remote row and does not replace the local row or remount its runtime.
  await page.eval(`window.__retainedComputerWindow = document.querySelector('.mc-frame').contentWindow`);
  await remote.rpc('Mutate', { op: 'renameChat', chatId: 'same-chat-id', title: 'Remote deployment updated' });
  await page.waitFor(`[...document.querySelectorAll('.mc-thread')].some(button => button.textContent.includes('Remote deployment updated'))`);
  assert.equal(await page.eval(`window.__retainedComputerWindow === document.querySelector('.mc-frame').contentWindow`), true);
  assert.equal(await page.eval(`window.__zuse.state.chats['same-chat-id'].title`), 'Local login');
  assert.equal(await page.eval(`document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input').value`), 'Keep the remote draft');
  // All source checkouts resolve to the same repository group when identity metadata arrives.
  const repositoryIds = await page.eval(`[window.__zuse.state.spaces['same-project-id'].repositoryId, document.querySelector('.mc-frame').contentWindow.__zuse.state.spaces['same-project-id'].repositoryId]`);
  assert.ok(repositoryIds[0] && repositoryIds[0] === repositoryIds[1], 'Both checkouts must report one repository identity');
  await page.waitFor(`document.querySelectorAll('.mc-project').length === 1`);
  await page.press('input[aria-label="Search across computers"]'); await page.type('deployment');
  await page.waitFor(`document.querySelectorAll('.mc-thread').length === 1 && document.querySelector('.mc-thread').textContent.includes('deployment')`);
  await page.eval(`(() => { const input = document.querySelector('.mc-search'); input.value = ''; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await page.waitFor(`document.querySelectorAll('.mc-thread').length === 2`);
  await page.key('2', 'Digit2', 50, 2);
  await page.waitFor(`document.querySelector('.mc-frame').hidden === false`);
  await page.eval(`document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input').focus()`);
  await page.key('1', 'Digit1', 49, 2);
  await page.waitFor(`document.querySelector('.mc-frame').hidden === true`);
  assert.equal(await page.eval(`document.querySelector('.mc-outlet:not([hidden]) .cmp-input').value`), 'Keep the local draft');
  await wait(350); // Wait for the debounced private browser registry and drafts.
  await page.goto(local.origin);
  await page.waitFor(`document.querySelectorAll('.mc-thread').length === 2 && document.querySelector('.mc-frame').contentWindow.__zuse?.state.workspaceSynced`, 30000);
  assert.equal(await page.eval(`document.querySelectorAll('iframe.mc-frame').length`), 1);
  await page.eval(`[...document.querySelectorAll('.mc-thread')].find(button => button.textContent.includes('Remote deployment updated')).click()`);
  await page.waitFor(`document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input')?.value === 'Keep the remote draft'`);
  if (process.env.ZUSE_SHOTS) await page.screenshot(join(process.env.ZUSE_SHOTS, 'multi-computer-sidebar.png'));
  await page.eval(`(() => { const input = document.querySelector('.mc-frame').contentDocument.querySelector('.cmp-input'); input.value = ''; input.dispatchEvent(new Event('input', { bubbles: true })); })()`);
  await wait(250);
  await page.click('.mc-computer-row:first-child .mc-computer-select');
  await page.waitFor(`document.querySelector('.mc-frame').hidden === true`);
  await page.click('.mc-computer-row:last-child .mc-computer-select');
  await page.waitFor(`document.querySelector('.mc-frame').hidden === false`);
  await page.click('.mc-computer-row button[aria-label^="Remove "]');
  await page.waitFor(`document.querySelectorAll('iframe.mc-frame').length === 0`);
  await page.waitFor(`document.querySelector('.cmp-input')?.value === 'Keep the local draft'`);
  await page.goto(local.origin);
  await page.waitFor(`window.__zuse?.state.workspaceSynced`);
  assert.equal(await page.eval(`document.querySelectorAll('iframe.mc-frame').length`), 0);
  assert.equal(page.errors.length, 0, page.errors.join('\n'));
  console.log('PASS two live engines: merged repository, scoped navigation with identical ids, isolated drafts, background updates, retained runtimes, shared keyboard navigation, search, reload restoration and removal.');
} catch (error) {
  if (page) {
    console.error('Browser errors:', page.errors.join('\n'));
    console.error('Visible text:', await page.eval(`document.body.innerText.slice(0, 2500)`).catch(() => 'unavailable'));
  }
  throw error;
} finally {
  await page?.close(); await local?.close(); await remote?.close(); rmSync(root, { recursive: true, force: true });
  if (!process.env.ZUSE_ASSETS) rmSync(assets, { recursive: true, force: true });
}
