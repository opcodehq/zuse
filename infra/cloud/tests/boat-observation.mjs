import test from 'node:test';
import assert from 'node:assert/strict';
import net from 'node:net';
import {mkdtemp,mkdir,writeFile,chmod,unlink,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {boatObservationCommand} from '../boat-observation.mjs';
const run=promisify(execFile);
test('readonly observation separates live process from replaced socket and still verifies retained data',async()=>{
 const root=await mkdtemp(join(tmpdir(),'zuse-boat-observe-')),data=join(root,'data'),ipc=join(data,'.runtime-supervision'),bin=join(root,'bin'),workspace=join(root,'workspace');
 const id='zuse-base-build-'+'a'.repeat(24),marker='retained-work-'+id,software=join(root,'program');
 await mkdir(ipc,{recursive:true});await mkdir(bin);await mkdir(workspace);
 await writeFile(join(workspace,'supervisor-dirty-work'),marker);await writeFile(software,'#!/bin/sh\nprintf "%s" '+marker+'\n');await chmod(software,0o555);
 await writeFile(join(bin,'systemctl'),'#!/bin/sh\nprintf "MainPID='+process.pid+'\\nActiveState=active\\nSubState=running\\nNRestarts=0\\nResult=success\\n"\n');await chmod(join(bin,'systemctl'),0o700);
 const address=join(ipc,'control.sock');const server=net.createServer(socket=>socket.once('data',()=>{socket.end(JSON.stringify({version:1,bound:false,childAlive:false,recoveryAlive:false,residentPid:process.pid,bootNonce:'private-fixture-nonce'})+'\n');}));
 await new Promise(resolve=>server.listen(address,resolve));
 try{
  let code=Buffer.from(boatObservationCommand(id).split(' ')[2],'base64').toString();
  code=code.replaceAll('/home/zuse/.zuse-data',data).replaceAll('/home/zuse/workspace',workspace).replaceAll('/usr/local/bin/zuse-boat-retention-canary',software);
  const observe=async()=>{const out=await run('python3',['-c',code],{env:{...process.env,PATH:bin+':'+process.env.PATH,BOAT_API_KEY:'must-never-be-emitted'},timeout:10000,maxBuffer:32768});assert.ok(!out.stdout.includes('must-never-be-emitted'));assert.ok(!out.stdout.includes(marker));return JSON.parse(out.stdout);};
  const before=await observe();assert.equal(before.rootPing.connected,true);assert.equal(before.paths.socket.kind,'socket');assert.equal(before.rootPing.ping.bootNonceSha256.length,64);assert.equal(before.dirtyData.matchesExpected,true);assert.equal(before.software.matchesExpected,true);
  await unlink(address);const after=await observe();
  assert.equal(server.listening,true);assert.equal(after.service.ActiveState,'active');assert.equal(after.residentProcess.pid,process.pid);assert.equal(after.paths.socket.error,'ENOENT');assert.equal(after.rootPing.connectError,'ENOENT');assert.ok(after.residentProcess.socketFds.length>0);assert.ok(after.boundUnixPath.length>0,'kernel bound path survives filesystem unlink');assert.equal(after.dirtyData.matchesExpected,true);assert.equal(after.software.matchesExpected,true);
 }finally{await new Promise(resolve=>server.close(resolve));await rm(root,{recursive:true});}
});
test('untrusted publication identity cannot become an executable observation command',()=>{
 assert.throws(()=>boatObservationCommand('anything; env'),/InvalidOriginalBuilderIdentity/);
});
