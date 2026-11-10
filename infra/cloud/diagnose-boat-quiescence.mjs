// Factory metadata-only stop classification. No runtime overlay or snapshot.
import{readFile,writeFile,mkdir}from'node:fs/promises';
import{dirname,resolve}from'node:path';
import{pathToFileURL}from'node:url';
import{build,verifyBundle,createBoatBuilderAdapter,requireFreshPublicationReport}from'./build-managed-base.mjs';
import{loadCleanLineage}from'./clean-base-lineage.mjs';
import{inspectOriginalBoat}from'./probe-boat-supervisor.mjs';
export async function diagnoseQuiescence(args,adapter,{buildBase=build}={}){
 await requireFreshPublicationReport(args.output);await requireFreshPublicationReport(args.output+'.builder.json');
 await mkdir(dirname(args.output),{recursive:true});
 const report={schema:1,provider:'boat',scope:'clean-parent-stop-unit-only',status:'incomplete',inspectionRequired:false,cleanupVerified:false,overlayInstalled:false,snapshotSubmitted:false,credentialsDelivered:false,agentAcceptanceVerified:false};
 await writeFile(args.output,JSON.stringify(report,null,2)+'\n',{mode:0o600,flag:'wx'});
 const result=await buildBase({...args,provider:'boat',output:args.output+'.builder.json',parentPreflightOnly:false,parentQuiescenceOnly:true,retainArtifact:false},adapter);
 Object.assign(report,{status:result.inspectionRequired?'observation-deferred':result.status==='parent-quiescence-verified'?'diagnosed':'failed',inspectionRequired:result.inspectionRequired===true,cleanupVerified:result.cleanupVerified===true,runId:result.runId,machineId:result.machineId,errorCode:result.errorCode,failedStage:result.failedStage,parentSupervisorHygiene:result.parentSupervisorHygiene});
 await writeFile(args.output,JSON.stringify(report,null,2)+'\n',{mode:0o600});return report;
}
async function main(){
 const argv=process.argv.slice(2),get=k=>argv[argv.indexOf(k)+1];
 if(argv.includes('--observe-existing')||argv.includes('--cleanup-existing')){const cleanup=argv.includes('--cleanup-existing');console.log(JSON.stringify(await inspectOriginalBoat(resolve(get(cleanup?'--cleanup-existing':'--observe-existing')),await createBoatBuilderAdapter(),{cleanup})));return;}
 if(!['--bundle','--output','--derive-lineage','--derive-lineage-sha256'].every(k=>argv.includes(k)))throw new Error('ArgumentsRequired');
 const args={bundle:resolve(get('--bundle')),output:resolve(get('--output')),deriveLineage:resolve(get('--derive-lineage')),deriveLineageSha256:get('--derive-lineage-sha256')};
 const bundle=await verifyBundle(args.bundle),parent=await loadCleanLineage(args.deriveLineage,args.deriveLineageSha256,bundle);
 if(parent.artifactId!=='zuse-base-build-2dd7fb58465b4c7999313fd8'||parent.providerArtifactVersion!=='c72c4d51-353f-4315-90d4-2b5dc69ded0c')throw new Error('OriginalParentMismatch');
 if(!argv.includes('--live')){console.log(JSON.stringify({status:'plan-only',scope:'clean-parent-stop-unit-only',providerEffects:false,parentArtifactId:parent.artifactId,parentVersion:parent.providerArtifactVersion,lifetimeSeconds:600,maxMachines:1,overlayInstalled:false,snapshotSubmitted:false}));return;}
 const result=await diagnoseQuiescence(args,await createBoatBuilderAdapter());console.log(JSON.stringify({status:result.status,cleanupVerified:result.cleanupVerified,inspectionRequired:result.inspectionRequired}));
}
if(process.argv[1]&&pathToFileURL(resolve(process.argv[1])).href===import.meta.url)main().catch(()=>{console.error('Clean parent diagnostic incomplete; inspect original receipts.');process.exitCode=1;});
