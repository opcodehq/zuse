#!/usr/bin/env python3
"""Release orchestration fixtures, never live provider or agent certification."""
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import sys
from types import SimpleNamespace
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / 'managed-base-release.py'
spec = importlib.util.spec_from_file_location('managed_base_release', SCRIPT)
tool = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tool)


class ManagedBaseReleaseTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.driver = self.root / 'driver'
        self.driver.write_text('''#!/usr/bin/env python3
import json,os,sys,time
r=json.load(sys.stdin)
with open(os.environ['ZUSE_CERT_TRACE'],'a') as out: out.write(json.dumps(r)+'\\n')
mode=os.environ.get('ZUSE_CERT_FAILURE')
if r['action']=='case' and mode=='timeout': time.sleep(3)
print(json.dumps(dict(runId=r['runId'],provider=r['provider'],releaseSha256=r['releaseSha256'],
 agent=r.get('agent'),case=r.get('case'),diagnostic='private fixture output',
 status='failed' if mode=='case' and r['action']=='case' else 'passed',
 remainingResources=1 if mode=='cleanup' else 0)))
''')
        self.driver.chmod(0o700)
        self.trace = self.root / 'trace'
        self.previous = {key: os.environ.get(key) for key in ('ZUSE_CERT_TRACE', 'ZUSE_CERT_FAILURE')}
        os.environ['ZUSE_CERT_TRACE'] = str(self.trace)
        os.environ.pop('ZUSE_CERT_FAILURE', None)
        self.addCleanup(self.restore_env)
        self.release = self.root / 'release.json'
        self.value = dict(provider='boxd', releaseId='fixture-base', artifactId='immutable-fixture',
                          architecture='x86_64', manifest=dict(managedBase=True, credentialsRemoved=True, repositories=[]),
                          capabilities=dict(id='boxd'), harnesses=[dict(agent=a, version='1.0.0', sha256='a'*64) for a in tool.AGENTS])
        tool.write(self.release, self.value)
        self.report = self.root / 'report.json'
        self.args = SimpleNamespace(release=self.release, driver=self.driver, timeout=2, output=self.report)

    def restore_env(self):
        for key, value in self.previous.items():
            if value is None: os.environ.pop(key, None)
            else: os.environ[key] = value

    def test_complete_matrix_is_bound_to_artifact_and_cleanup(self):
        result = tool.certify(self.args)
        self.assertEqual(len(result['cases']), 33)
        self.assertTrue(result['cleanupVerified'])
        self.assertNotIn('private fixture output', self.report.read_text())
        publication = SimpleNamespace(release=self.release, report=self.report, output=self.root/'published.json')
        self.assertEqual(tool.publication(publication)['status'], 'review-required')
        self.assertEqual(tool.load(publication.output)['certificationReportSha256'], tool.sha(self.report.read_bytes()))
        self.value['artifactId'] = 'different-artifact'
        tool.write(self.release, self.value)
        with self.assertRaisesRegex(ValueError, 'artifact_bound'):
            tool.publication(publication)

    def test_failed_case_and_timeout_always_reconcile_cleanup(self):
        for mode in ('case', 'timeout'):
            os.environ['ZUSE_CERT_FAILURE'] = mode
            self.args.timeout = 1
            with self.assertRaises(ValueError): tool.certify(self.args)
            report = tool.load(self.report)
            self.assertEqual(report['status'], 'failed')
            self.assertTrue(report['cleanupVerified'])
            requests = [json.loads(line) for line in self.trace.read_text().splitlines()]
            self.assertEqual(requests[-1]['action'], 'cleanup')
            self.assertEqual(requests[-1]['runId'], report['runId'])

    def test_uncertain_cleanup_cannot_publish(self):
        os.environ['ZUSE_CERT_FAILURE'] = 'cleanup'
        with self.assertRaisesRegex(ValueError, 'cleanup_required'): tool.certify(self.args)
        self.assertEqual(tool.load(self.report)['status'], 'cleanup-required')
        with self.assertRaises(ValueError):
            tool.publication(SimpleNamespace(release=self.release, report=self.report, output=self.root/'published.json'))

    def test_context_copies_only_pinned_native_allowlist(self):
        root = self.root/'harnesses'
        entries = {a: dict(version='1.0.0', sha256=tool.sha(Path(sys.executable).resolve().read_bytes())) for a in tool.AGENTS}
        manifest = dict(schema=2, platform='linux-'+platform.machine(), adapterRevision='a'*40,
                        runtimeVersion='0.25.1', stateFormat='fixture', harnesses=entries)
        identifier = tool.sha(tool.canonical(manifest))
        source = root/'releases'/identifier
        source.mkdir(parents=True)
        for agent in tool.AGENTS:
            shutil.copyfile(Path(sys.executable).resolve(), source/agent)
            (source/agent).chmod(0o555)
        tool.write(source/'manifest.json', manifest)
        (source/'unexpected-secret').write_text('must never copy')
        (root/'.release-lock').touch()
        args = SimpleNamespace(harness_root=root, harness_release=identifier,
                               runtime_image='registry.example/runtime@sha256:'+'b'*64, output=self.root/'context')
        result = tool.context(args)
        self.assertEqual(result['status'], 'candidate-only')
        self.assertFalse((args.output/'harnesses'/'releases'/identifier/'unexpected-secret').exists())
        self.assertIn('USER 10001:10001', (args.output/'Dockerfile').read_text())
        args.output = self.root/'unsafe'
        args.runtime_image = 'registry.example/runtime:latest'
        with self.assertRaisesRegex(ValueError, 'immutable_runtime'): tool.context(args)


    def test_staging_candidate_requires_retained_artifact_and_exact_installed_pins(self):
        bundle=self.root/'bundle';bundle.mkdir()
        names=('runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json')
        for name in names:(bundle/name).write_bytes(b'fixture-public-native-'+name.encode())
        pins={agent:dict(version='fixture-1',sha256=tool.file_sha(bundle/agent)) for agent in tool.AGENTS}
        harness=dict(schema=2,platform='linux-x86_64',harnesses=pins)
        tool.write(bundle/'harness-manifest.json',harness)
        runtime_sha=tool.file_sha(bundle/'runtime')
        tool.write(bundle/'bundle.json',dict(schema=1,status='candidate-only',releaseId='fixture-base',runtimeSha256=runtime_sha,harnessRelease=tool.sha(tool.canonical(harness)),platform='linux-x86_64',files=[dict(path=n,bytes=(bundle/n).stat().st_size,sha256=tool.file_sha(bundle/n)) for n in names]))
        manifest=dict(managedBase=True,credentialsRemoved=True,repositories=[],rustRuntimeSupervisorVersion=1,buildId='fixture-base')
        build=self.root/'build-proof.json';proof=self.root/'mechanism-proof.json'
        tool.write(build,dict(provider='boxd',status='prepared-uncertified',cleanupVerified=True,artifactRetained=True,artifactId='original-snapshot',runtimeSha256=runtime_sha,manifest=manifest))
        value=dict(status='mechanism-verified-uncertified',cleanupVerified=True,artifactRetained=True,artifactId='original-snapshot',candidateArtifactStatus='uncertified-staging')
        tool.write(proof,value)
        caps=self.root/'caps.json';catalogs=self.root/'catalogs.json'
        tool.write(caps,dict(id='boxd'))
        tool.write(catalogs,{a:dict(id='claude-code' if a=='claude' else a,models=[dict(id='fixture')]) for a in tool.AGENTS})
        args=SimpleNamespace(build_report=build,proof=proof,bundle=bundle,capabilities=caps,catalogs=catalogs,output=self.root/'candidate.json')
        self.assertEqual(tool.candidate(args)['status'],'uncertified-staging-candidate')
        result=tool.load(args.output);self.assertNotIn('certificationReportSha256',result)
        # New stopped-memory images require the actual shared restored-machine
        # bootstrap proof. An optional source flag alone cannot promote them.
        original_bundle=tool.load(bundle/'bundle.json');original_build=tool.load(build);original_proof=tool.load(proof)
        helper=bundle/'managed-base-fresh-boot.py';helper.write_bytes(b'fixture fresh bootstrap')
        fresh=json.loads(json.dumps(original_bundle));fresh['managedBaseFreshBootVersion']=1
        fresh['files'].append(dict(path=helper.name,bytes=helper.stat().st_size,sha256=tool.file_sha(helper)))
        tool.write(bundle/'bundle.json',fresh)
        spec=dict(version=1,mode='stopped-memory-snapshot-v1',runtimeSha256=runtime_sha,
                  supervisorSha256=tool.file_sha(bundle/'runtime-supervisor.py'),helperSha256=tool.file_sha(helper),serviceSha256='a'*64,manifestSha256='b'*64)
        request=dict(version=1,provider='boxd',purpose='publication-proof',operationId='original-op',workspaceId='image-proof',generation=1,storageIncarnationId='image-disk',sourceMachineId='restored-original',imagePins={k:v for k,v in spec.items() if k.endswith('Sha256')})
        built=dict(original_build,runId='original-op',initialRuntimeBootstrap=spec)
        observed=dict(request=request,receipt=dict(request,status='ready',requestSha256=tool.sha(tool.canonical(request)),resident=dict(ipcVerified=True,unbound=True,childAlive=False,recoveryAlive=False,residentPid=42,residentStartTicks=123,bootNonceSha256="c"*64)))
        proved=dict(original_proof,restoreMachineId='restored-original',initialRuntimeBootstrap=spec,initialRuntimeBootstrapReceipt=observed)
        tool.write(build,built);tool.write(proof,proved);args.output=self.root/'fresh-candidate.json'
        tool.candidate(args);self.assertEqual(tool.load(args.output)['initialRuntimeBootstrap'],spec)
        for name,mutate in [('missing',lambda v:v.pop('initialRuntimeBootstrapReceipt')),('wrong machine',lambda v:v['initialRuntimeBootstrapReceipt']['request'].update(sourceMachineId='other')),('bound',lambda v:v['initialRuntimeBootstrapReceipt']['receipt']['resident'].update(unbound=False))]:
            changed=json.loads(json.dumps(proved));mutate(changed);tool.write(proof,changed);args.output=self.root/'fresh-rejected.json'
            with self.subTest(name=name),self.assertRaisesRegex(ValueError,'fresh_boot_actual_restore'):tool.candidate(args)
        # Boat uses an explicit distinct mode and requires both disabled capture
        # and clean cold-start evidence; it never claims retained task work.
        boat_fresh=json.loads(json.dumps(fresh));boat_fresh['managedBaseFreshBootMode']='post-hydration-first-start-v1'
        boat_spec={**spec,'mode':'post-hydration-first-start-v1'};boat_request={**request,'provider':'boat'}
        boat_observed=dict(request=boat_request,receipt={**observed['receipt'],**boat_request,'requestSha256':tool.sha(tool.canonical(boat_request))})
        boat_build={**built,'provider':'boat','initialRuntimeBootstrap':boat_spec,'disabledBootCapture':dict(unitDisabled=True,operationId='original-op',runtimeSha256=runtime_sha,helperSha256=spec['helperSha256'],serviceSha256=spec['serviceSha256'])}
        boat_proof={**proved,'initialRuntimeBootstrap':boat_spec,'initialRuntimeBootstrapReceipt':boat_observed,'unitRemainsDisabled':True,'cleanColdRestartVerified':True,'taskRetentionVerified':False}
        tool.write(bundle/'bundle.json',boat_fresh);tool.write(build,boat_build);tool.write(proof,boat_proof);tool.write(caps,dict(id='boat'));args.output=self.root/'boat-candidate.json'
        tool.candidate(args);self.assertEqual(tool.load(args.output)['initialRuntimeBootstrap']['mode'],'post-hydration-first-start-v1')
        for patch in [dict(unitRemainsDisabled=False),dict(cleanColdRestartVerified=False),dict(taskRetentionVerified=True)]:
            tool.write(proof,{**boat_proof,**patch});args.output=self.root/'boat-rejected.json'
            with self.assertRaisesRegex(ValueError,'disabled_boot_capture'):tool.candidate(args)
        tool.write(caps,dict(id='boxd'))
        # Legacy images remain unchanged and cannot silently carry the feature.
        tool.write(bundle/'bundle.json',original_bundle);tool.write(build,original_build);tool.write(proof,original_proof)
        args.output=self.root/'candidate.json'
        self.assertEqual(result['runtimeSha256'],runtime_sha);self.assertEqual(len(result['harnesses']),3)
        args.output=self.root/'claude-candidate.json';tool.write(catalogs,dict(claude=tool.load(catalogs)['claude']))
        tool.candidate(args);claude=tool.load(args.output)
        self.assertEqual([h['agent'] for h in claude['harnesses']],['claude'])
        self.assertEqual(claude['artifactId'],result['artifactId']);self.assertNotEqual(claude['releaseId'],result['releaseId'])
        self.assertEqual(claude['manifest']['buildId'],'fixture-base')
        # Existing immutable eleven-file candidates can revise discovered model
        # metadata without rebuilding; they cannot acquire a missing extension.
        legacy=tool.load(bundle/'bundle.json')
        legacy['files']=[f for f in legacy['files'] if f['path'] not in ('runtime-repair.py','runtime-recreation.py')]
        tool.write(bundle/'bundle.json',legacy)
        args.output=self.root/'legacy-candidate.json';tool.candidate(args)
        self.assertEqual(tool.load(args.output)['artifactId'],'original-snapshot')
        args.output=self.root/'legacy-extension-invalid.json'
        updated=tool.load(build);updated['manifest']['rustRuntimeRepairVersion']=1;tool.write(build,updated)
        with self.assertRaisesRegex(ValueError,'extension_bundle_helpers'):tool.candidate(args)
        del updated['manifest']['rustRuntimeRepairVersion'];tool.write(build,updated)
        args.output=self.root/'deleted-candidate.json';value['artifactRetained']=False;tool.write(proof,value)
        with self.assertRaisesRegex(ValueError,'retained_candidate'):tool.candidate(args)
        value['artifactRetained']=True;tool.write(proof,value);(bundle/'codex').write_text('changed-native-bits')
        with self.assertRaisesRegex(ValueError,'integrity'):tool.candidate(args)

    def test_disabled_parent_candidate_uses_distinct_verified_receipt(self):
        import copy
        spec = importlib.util.spec_from_file_location('disabled_candidate_fixture', Path(__file__).with_name('managed-base-derive.py'))
        fixture = importlib.util.module_from_spec(spec); spec.loader.exec_module(fixture)
        temp = tempfile.TemporaryDirectory(dir=fixture.ROOT/'.context', prefix='candidate-disabled-test-')
        self.addCleanup(temp.cleanup)
        f = fixture.fixture(Path(temp.name)); service = fixture.disabled_fixture(f)
        D = fixture.D
        _, lineage, bundle, _, _ = D.context(f['input'], f['expected'], f['owner'])
        image = D.verify_parent(f['input'], f['expected'], filesystem=f['image'], owner=f['owner'], quiescence_image=True, service_fact=service, process_facts=[])
        Q = fixture.module('disabled_candidate_quiescence', fixture.ROOT/'infra/cloud/clean-parent-quiescence.py')
        receipt = Q.already_disabled_parent(image, 'owned-operation', f['expected'], lambda: image)
        build = dict(runId='owned-operation', runtimeSha256=bundle['runtimeSha256'],
                     cleanParent=dict(artifactId=lineage['builder']['artifactId'], parentRuntimeSha256=lineage['builder']['runtimeSha256'], lineageSha256=f['expected'], harnessRelease=bundle['harnessRelease']),
                     parentSupervisorHygiene=receipt, stoppedParentPreflight=dict(image, status='passed'))
        self.assertTrue(tool.disabled_parent_hygiene(build, lineage, D))
        for field, value in [('alreadyDisabledParent', False), ('legacyBuilder', True), ('atomicStopAcknowledged', True), ('startedOrStoppedProcess', True), ('noAuthorityPresent', False), ('noResidentPresent', False), ('enabledBootServiceRetained', True), ('operationId', 'other'), ('runtimeSha256', '0'*64), ('bootServiceSha256', '0'*64), ('residentPid', 123)]:
            changed = copy.deepcopy(build); changed['parentSupervisorHygiene'][field] = value
            with self.subTest(field=field): self.assertFalse(tool.disabled_parent_hygiene(changed, lineage, D))
        for field in ('parentImageProof', 'stoppedParentPreflight'):
            changed = copy.deepcopy(build)
            target = changed['parentSupervisorHygiene'][field] if field == 'parentImageProof' else changed[field]
            target['parentRuntimeSha256'] = '0'*64
            with self.subTest(field=field): self.assertFalse(tool.disabled_parent_hygiene(changed, lineage, D))

    def test_derived_candidate_requires_exact_clean_parent_lineage_and_both_hygiene_receipts(self):
        spec=importlib.util.spec_from_file_location('candidate_derive_fixture',Path(__file__).with_name('managed-base-derive.py'))
        fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
        f=fixture.fixture(self.root);bundle=f['input'];target=f['target'];target['platform']='linux-x86_64'
        plan=dict(buildId='installed-plan-build',templateVersion='installed-plan-build',configurationDigest='installed-plan-build');tool.write(bundle/'prepare-plan.json',plan)
        target['files']=[dict(path=e['path'],bytes=(bundle/e['path']).stat().st_size,sha256=tool.file_sha(bundle/e['path'])) if e['path']=='prepare-plan.json' else e for e in target['files']];tool.write(bundle/'bundle.json',target)
        tool.write(bundle/'parent-lineage.json',f['lineage']);lineage=tool.load(bundle/'parent-lineage.json');parent=json.loads(lineage['documents']['builder'])
        old=dict(legacyBuilder=True,operationId='new-operation',parentLineageSha256=tool.file_sha(bundle/'parent-lineage.json'),runtimeDataAbsent=True,originalResidentExited=True,supervisorLockReleased=True)
        fresh=dict(legacyBuilder=False,atomicStopAcknowledged=True,operationId='new-operation',runtimeSha256=target['runtimeSha256'],runtimeDataAbsent=True,originalResidentExited=True,supervisorLockReleased=True)
        build=dict(provider='boat',status='prepared-uncertified',cleanupVerified=True,artifactRetained=True,artifactId='new-snapshot',runId='new-operation',releaseId=target['releaseId'],runtimeSha256=target['runtimeSha256'],manifest=dict(managedBase=True,credentialsRemoved=True,repositories=[],rustRuntimeSupervisorVersion=1,**plan),bundleSha256=tool.file_sha(bundle/'bundle.json'),cleanParent=dict(lineageSha256=tool.file_sha(bundle/'parent-lineage.json'),artifactId=parent['artifactId'],providerArtifactVersion=parent['providerArtifactVersion'],parentRuntimeSha256=parent['runtimeSha256'],parentBundleSha256=lineage['parentBundleFileSha256'],harnessRelease=target['harnessRelease'],reusedFiles=sorted(fixture.D.SKIP)),parentSupervisorHygiene=old,supervisorHygiene=fresh,stoppedParentPreflight=dict(status='passed'))
        proof=dict(status='mechanism-verified-uncertified',cleanupVerified=True,artifactRetained=True,artifactId='new-snapshot',candidateArtifactStatus='uncertified-staging',sourceMachineAbsent=True,runtimeSha256=target['runtimeSha256'])
        args=SimpleNamespace(bundle=bundle,build_report=self.root/'derived-build.json',proof=self.root/'derived-proof.json',capabilities=self.root/'derived-caps.json',catalogs=self.root/'derived-catalogs.json',output=self.root/'derived-candidate.json')
        tool.write(args.build_report,build);tool.write(args.proof,proof);tool.write(args.capabilities,dict(id='boat'));tool.write(args.catalogs,{a:dict(id='claude-code' if a=='claude' else a,models=[dict(id='fixture')]) for a in tool.AGENTS})
        self.assertEqual(tool.candidate(args)['status'],'uncertified-staging-candidate')
        self.assertEqual(tool.load(args.output)['artifactId'],'new-snapshot')
        args.output=self.root/'derived-invalid.json'
        for name,mutate,reason in [
            ('missing version',lambda v:v.pop('managedBaseDerivationVersion'),'allowlist'),
            ('wrong version',lambda v:v.update(managedBaseDerivationVersion=2),'allowlist')]:
            changed=json.loads(json.dumps(target));mutate(changed);tool.write(bundle/'bundle.json',changed)
            with self.subTest(name=name),self.assertRaisesRegex(ValueError,reason):tool.candidate(args)
        tool.write(bundle/'bundle.json',target)
        for name,mutate in [
            ('parent version',lambda v:v['cleanParent'].update(providerArtifactVersion='wrong')),
            ('false reused payload',lambda v:v['cleanParent']['reusedFiles'].append('harness-manifest.json')),
            ('wrong derived bundle ID',lambda v:v.update(releaseId='wrong')),
            ('wrong installed plan',lambda v:v['manifest'].update(templateVersion='wrong')),
            ('unbound bundle',lambda v:v.update(bundleSha256='0'*64)),
            ('lineage digest',lambda v:v['cleanParent'].update(lineageSha256='0'*64)),
            ('missing atomic stop',lambda v:v['supervisorHygiene'].update(atomicStopAcknowledged=False)),
            ('wrong new operation',lambda v:v['supervisorHygiene'].update(operationId='other')),
            ('task data',lambda v:v['parentSupervisorHygiene'].update(runtimeDataAbsent=False))]:
            changed=json.loads(json.dumps(build));mutate(changed);tool.write(args.build_report,changed)
            with self.subTest(name=name),self.assertRaisesRegex(ValueError,'candidate_derivation_'):tool.candidate(args)
        tool.write(args.build_report,build)
        path=bundle/'parent-lineage.json';path.unlink()
        with self.assertRaisesRegex(ValueError,'lineage_required'):tool.candidate(args)
        tool.write(path,lineage)
        changed=json.loads(json.dumps(lineage));changed['documents']['proof']+=' '
        tool.write(path,changed);updated=json.loads(json.dumps(build));updated['cleanParent']['lineageSha256']=tool.file_sha(path);updated['parentSupervisorHygiene']['parentLineageSha256']=tool.file_sha(path);tool.write(args.build_report,updated)
        with self.assertRaisesRegex(ValueError,'parent_lineage_digest_mismatch'):tool.candidate(args)
        tool.write(path,lineage);tool.write(args.build_report,build)
        (bundle/'managed-base-derive.py').write_text('tampered helper')
        with self.assertRaisesRegex(ValueError,'bundle_integrity_failed'):tool.candidate(args)

    def test_portable_reports_require_exact_original_build_and_real_readiness(self):
        bundle=dict(runtimeSha256='a'*64,harnessRelease='b'*64)
        gh=dict(status='passed',required=True,canonicalPath='/opt/zuse/bin/gh',ancestorsControlled=True,profileMatches=True,loginShellMatchesGlobal=True,globalVersionMatchesReceipt=True)
        common=dict(status='prepared-uncertified',certification=False,registration=False,modelCredentials=False,retainArtifact=True,runtimeSha256=bundle['runtimeSha256'],harnessRelease=bundle['harnessRelease'],bundleSha256='c'*64,helperSha256='d'*64,builderSha256='e'*64,operationId='original-op',residentRestore=dict(status='resident-ready',runtimeSha256=bundle['runtimeSha256']),githubCliProof=gh)
        records=[dict(common,provider='e2b',sdkVersion='2.62.0',sdkIntegrity='sha512-RKYzGLgFLJa9xCct6Bh255UbERo/yaYqG/3sUqau60sZ38TFZSAt2M+eXGfKWo3yD31iu9iNVC44H0x9ppaoJw==',artifactId='original-template',templateId='original-template',providerArtifactVersion='original-build',buildId='original-build'),dict(common,provider='daytona',sdkVersion='0.227.0',sdkIntegrity='sha512-KxSKEOew52nUvi+oJIaTOEnDV1MQJ6j6JafmzguD6tKgBhwJM0k9C/jUGwSiksxdjzx5FFr1sSVTwhtd3uZE7Q==',artifactId='original-snapshot',snapshotId='original-snapshot',providerArtifactVersion='original-snapshot',sandboxClass='container',memoryPreservationVerified=False,diskRetentionVerified=True,coldResidentRestore=dict(status='resident-ready',runtimeSha256=bundle['runtimeSha256']),dockerfileSha256='f'*64)]
        records[0].update(name='original-private-build',manifestSha256='1'*64,bootSha256='2'*64,residentRestore=dict(common['residentRestore'],bootSealVerified=True,bootSha256='2'*64))
        records[0]['providerArtifactBinding']=dict(version=1,mode='private-single-build',artifactId='original-template',artifactVersion='original-build',artifactName='original-private-build',runtimeSha256=bundle['runtimeSha256'],manifestSha256='1'*64)
        import copy
        for record in records:
            tool.portable_candidate_report(record,copy.deepcopy(record),bundle)
            for field,value in [('certification',True),('registration',True),('retainArtifact',False),('sdkVersion','unreviewed'),('helperSha256','bad'),('residentRestore',{}),('githubCliProof',dict(gh,canonicalPath='/usr/local/bin/gh'))]:
                changed=copy.deepcopy(record);changed[field]=value
                with self.assertRaisesRegex(ValueError,'portable_'):tool.portable_candidate_report(record,changed,bundle)
            changed=copy.deepcopy(record);changed['operationId']='replacement'
            with self.assertRaisesRegex(ValueError,'original_operation'):tool.portable_candidate_report(record,changed,bundle)
        altered=dict(records[0],providerArtifactBinding=dict(records[0]['providerArtifactBinding'],artifactVersion='different-build'))
        with self.assertRaisesRegex(ValueError,'private_build'):tool.portable_candidate_report(altered,altered,bundle)
        for patch in [dict(bootSha256=''),dict(bootSha256='invalid'),dict(residentRestore=dict(records[0]['residentRestore'],bootSealVerified=False)),dict(residentRestore=dict(records[0]['residentRestore'],bootSha256='3'*64))]:
            changed=dict(records[0],**patch)
            with self.assertRaisesRegex(ValueError,'managed_boot_seal'):tool.portable_candidate_report(changed,changed,bundle)
        changed=copy.deepcopy(records[0]);changed['bootSha256']='3'*64;changed['residentRestore']['bootSha256']='3'*64
        with self.assertRaisesRegex(ValueError,'original_operation'):tool.portable_candidate_report(records[0],changed,bundle)
        for patch in [dict(memoryPreservationVerified=True),dict(sandboxClass='linux-vm'),dict(diskRetentionVerified=False),dict(providerArtifactVersion='replacement')]:
            changed=dict(records[1],**patch)
            with self.assertRaisesRegex(ValueError,'persistent_container'):tool.portable_candidate_report(changed,changed,bundle)

    def test_exact_sequential_e2b_sdk_derivation_preserves_private_build_gates(self):
        import copy
        bundle=dict(runtimeSha256='a'*64,harnessRelease='b'*64)
        record=dict(provider='e2b',status='prepared-uncertified',certification=False,registration=False,modelCredentials=False,retainArtifact=True,runtimeSha256='a'*64,harnessRelease='b'*64,bundleSha256='c'*64,helperSha256='d'*64,builderSha256='e'*64,bootSha256='2'*64,operationId='original-op',artifactId='original-template',templateId='original-template',providerArtifactVersion='original-build',buildId='original-build',name='original-private-build',manifestSha256='1'*64,sdkVersion='2.62.0',sdkDerivation='zuse-sequential-upload-v1',sdkUpstreamModuleSha256='1c96b58fb06dd23f45d869a20800356b8712e33ff6c7b3bd04034e2fac112eaa',sdkModuleSha256='b69736a75bfdc0512683b55b38b971e4294f2382c2d2e820d819a8f3ad2242f9',sdkUpstreamIntegrity='sha512-RKYzGLgFLJa9xCct6Bh255UbERo/yaYqG/3sUqau60sZ38TFZSAt2M+eXGfKWo3yD31iu9iNVC44H0x9ppaoJw==',residentRestore=dict(status='resident-ready',runtimeSha256='a'*64,bootSealVerified=True,bootSha256='2'*64),githubCliProof=dict(status='passed',required=True,canonicalPath='/opt/zuse/bin/gh',ancestorsControlled=True,profileMatches=True,loginShellMatchesGlobal=True,globalVersionMatchesReceipt=True))
        record['providerArtifactBinding']=dict(version=1,mode='private-single-build',artifactId=record['artifactId'],artifactVersion=record['buildId'],artifactName=record['name'],runtimeSha256='a'*64,manifestSha256='1'*64)
        tool.portable_candidate_report(record,copy.deepcopy(record),bundle)
        for key,value in [('sdkDerivation','unreviewed'),('sdkModuleSha256','f'*64),('sdkUpstreamModuleSha256','f'*64),('sdkUpstreamIntegrity','wrong'),('sdkVersion','unreviewed'),('sdkIntegrity',record['sdkUpstreamIntegrity'])]:
            changed=copy.deepcopy(record);changed[key]=value
            with self.assertRaisesRegex(ValueError,'portable_'):tool.portable_candidate_report(changed,changed,bundle)
        missing=copy.deepcopy(record);missing.pop('sdkDerivation')
        with self.assertRaisesRegex(ValueError,'portable_'):tool.portable_candidate_report(missing,missing,bundle)
        stock=copy.deepcopy(record)
        for key in ['sdkDerivation','sdkModuleSha256','sdkUpstreamModuleSha256','sdkUpstreamIntegrity']:stock.pop(key)
        stock['sdkIntegrity']=record['sdkUpstreamIntegrity'];tool.portable_candidate_report(stock,copy.deepcopy(stock),bundle)
        mixed=copy.deepcopy(stock);mixed['sdkUpstreamModuleSha256']='f'*64
        with self.assertRaisesRegex(ValueError,'portable_'):tool.portable_candidate_report(mixed,mixed,bundle)
        with self.assertRaisesRegex(ValueError,'original_operation'):tool.portable_candidate_report(record,stock,bundle)
        changed=copy.deepcopy(record);changed['providerArtifactBinding']['artifactVersion']='different'
        with self.assertRaisesRegex(ValueError,'private_build'):tool.portable_candidate_report(changed,changed,bundle)

if __name__ == '__main__': unittest.main()
