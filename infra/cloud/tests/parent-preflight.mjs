import test from 'node:test';
import assert from 'node:assert/strict';
import {copyFile,mkdtemp,writeFile,readFile,stat,readdir,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {execFile} from 'node:child_process';
import {promisify} from 'node:util';
import {parentPreflightCommand,removeParentPreflight,persistStoppedParent} from '../parent-preflight.mjs';
const exec=promisify(execFile);

test('read-only parent import leaves no bytecode that breaks exact input cleanup',async()=>{
 const root=await mkdtemp(join(tmpdir(),'zuse-parent-preflight-'));
 try{
  await copyFile(new URL('../harness-release.py',import.meta.url),join(root,'harness-release.py'));
  for(const name of ['bundle.json','harness-manifest.json','derivation.json'])await writeFile(join(root,name),'{}');
  // Use the real imported manager and the production inspection launcher. This
  // used to create __pycache__, making the production rmdir fail after upload.
  const source="import importlib.util,sys\nfrom pathlib import Path\np=Path(sys.argv[sys.argv.index('--input')+1])/'harness-release.py'\ns=importlib.util.spec_from_file_location('parent_harness',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)";
  await exec('/bin/sh',['-c',parentPreflightCommand(source,root,'a'.repeat(64),'b'.repeat(64))]);
  const adapter={exec:async(_id,command)=>(await exec('/bin/sh',['-c',command.replaceAll('/var/lib/zuse/base-preflight',root)])).stdout};
  await removeParentPreflight(adapter,'owned-builder','sh -c');
  await assert.rejects(readdir(root),{code:'ENOENT'});
 }finally{await rm(root,{recursive:true,force:true});}
});

test('stopped receipt persists exact original operation and refuses replacement',async()=>{
 const root=await mkdtemp(join(tmpdir(),'zuse-stopped-parent-'));
 try{
  await writeFile(join(root,'bundle.json'),'target-bundle');
  const adapter={exec:async(_id,command)=>(await exec('/bin/sh',['-c',command.replaceAll('/var/lib/zuse/base-preflight',root)])).stdout};
  const args={adapter,id:'original-builder',rootShell:'sh -c',parent:{sha256:'a'.repeat(64)},bundle:root,operationId:'original-op',hygiene:{runtimeSha256:'b'.repeat(64)}};
  const receipt=await persistStoppedParent(args);
  assert.deepEqual(JSON.parse(await readFile(join(root,'parent-stopped.json'),'utf8')),receipt);
  assert.equal((await stat(join(root,'parent-stopped.json'))).mode&0o777,0o600);
  assert.equal(receipt.operationId,'original-op');assert.equal(receipt.machineId,'original-builder');
  await assert.rejects(persistStoppedParent({...args,operationId:'replacement'}));
 }finally{await rm(root,{recursive:true,force:true});}
});
