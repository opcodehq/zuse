import test from 'node:test';import assert from 'node:assert/strict';import{mkdtemp,rm,readFile}from'node:fs/promises';import{tmpdir}from'node:os';import{join}from'node:path';import{probeCleanRoot}from'../probe-clean-root.mjs';
for(const mode of ['observed','lost-create','not-ready'])test('ownership-only original diagnostic '+mode,async()=>{
 const root=await mkdtemp(join(tmpdir(),'root-diagnostic-'));let active=false,creates=0,deletes=0,name;const commands=[];
 const adapter={snapshotState:async()=>({name:'parent',snapshotId:'version',status:'ready'}),createFromCleanSnapshot:async(n,p,options)=>{assert.equal(options.ttlSeconds,600);assert.equal(p,'parent');name=n;creates++;active=true;if(mode==='lost-create')throw new Error('UncertainCreate');return'original';},find:async n=>{assert.equal(n,name);return active?'original':undefined;},ready:async()=>mode!=='not-ready',exec:async(id,cmd)=>{commands.push(cmd);return cmd==='id -u'?'1000':cmd==='sudo -n id -u'?'0':cmd.includes('os.lstat')?JSON.stringify({'/opt':{uid:1000,mode:'0o775',symlink:false}}):'';},identity:async()=>({id:'original',name}),remove:async()=>{active=false;deletes++;},absent:async()=>!active,close:()=>{}};
 try{const r=await probeCleanRoot({output:join(root,'report.json'),parentArtifactId:'parent',parentProviderArtifactVersion:'version',readyTimeoutMs:mode==='not-ready'?1:100},adapter);assert.equal(creates,1);
  if(mode==='not-ready'){assert.equal(r.status,'observation-deferred');assert.equal(deletes,0);assert.equal(r.cleanupVerified,false);assert.equal(active,true);}
  else{assert.equal(deletes,1);assert.equal(r.cleanupVerified,true);assert.equal(r.status,mode==='observed'?'observed':'failed');}
  if(mode==='observed'){assert.equal(r.before['/opt'].uid,1000);assert.equal(r.after['/opt'].mode,'0o775');assert.ok(commands.some(c=>c.includes('chown -hR 0:0')));assert.ok(!commands.some(c=>/prepare-image|launch-rust-runtime|apt-get/.test(c)));}
 }finally{await rm(root,{recursive:true});}
});
