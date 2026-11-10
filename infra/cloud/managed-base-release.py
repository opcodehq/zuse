#!/usr/bin/env python3
"""Assemble clean pinned base contexts and run bounded operator certification drivers.

This tool never registers a release or fabricates provider build/import support.
Drivers are trusted operator/CI programs, not a signature or entitlement boundary.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import tempfile
import time
import uuid

PROVIDERS = ('boxd', 'boat', 'e2b', 'daytona')
AGENTS = ('claude', 'codex', 'grok')
CASES = ('prepared-create', 'cache-miss-create', 'stream-tools-edit-test',
         'stop-followup', 'closed-app', 'reconnect-no-duplicate', 'sleep-resume',
         'process-loss-renewal', 'terminal-files-preview', 'git-push-pr', 'recreate-delete')
KEYS = {'boxd': 'BOXD_API_KEY', 'boat': 'BOAT_API_KEY', 'e2b': 'E2B_API_KEY', 'daytona': 'DAYTONA_API_KEY'}
DIGEST = re.compile(r'[0-9a-f]{64}\Z')


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def sha(value):
    return hashlib.sha256(value).hexdigest()


def file_sha(path):
    digest = hashlib.sha256()
    with path.open('rb') as source:
        while block := source.read(1024 * 1024):
            digest.update(block)
    return digest.hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(dir=path.parent, prefix='.release-', delete=False) as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(canonical(value) + b'\n')
        output.flush()
        os.fsync(output.fileno())
        name = output.name
    os.replace(name, path)


def load(path):
    if path.stat().st_size > 256 * 1024:
        raise ValueError('release_document_too_large')
    return json.loads(path.read_bytes())


def context(args):
    if not re.fullmatch(r'[^\s]+@sha256:[0-9a-f]{64}', args.runtime_image):
        raise ValueError('immutable_runtime_image_required')
    if args.output.exists():
        raise ValueError('new_empty_context_directory_required')
    spec = importlib.util.spec_from_file_location('harness_release', Path(__file__).with_name('harness-release.py'))
    manager = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(manager)
    with manager.locked(args.harness_root, read_only=True):
        directory, manifest = manager.release(args.harness_root, args.harness_release, verify=True)
        if manifest['schema'] != 2:
            raise ValueError('all_three_pinned_harnesses_required')
        # Context is an allowlist, never a copy of HOME, task disk or auth profiles.
        args.output.mkdir(mode=0o700, parents=True)
        target = args.output / 'harnesses'
        (target / 'releases').mkdir(parents=True)
        selected = target / 'releases' / args.harness_release
        selected.mkdir()
        if not manifest['platform'].startswith('linux-'):
            raise ValueError('linux_native_harnesses_required')
        for agent in AGENTS:
            source = directory / agent
            with source.open('rb') as executable:
                if executable.read(4) != b'\x7fELF':
                    raise ValueError('standalone_linux_executables_required')
            shutil.copyfile(source, selected / agent)
            (selected / agent).chmod(0o555)
        if manifest['harnesses']['codex'].get('assets'):
            manager.copy_assets(directory,selected,manifest['harnesses']['codex']['assets'])
            manager.complete_codex(selected,manifest)
        shutil.copyfile(directory / 'manifest.json', selected / 'manifest.json')
        (target / 'channels').mkdir()
        manager.atomic_json(target / 'channels' / 'staging.json', {'release': args.harness_release, 'previous': None})
        (target / '.release-lock').touch(mode=0o644)
        dockerfile = ('FROM ' + args.runtime_image + '\nUSER 0:0\n'
            'COPY --chown=0:0 harnesses/ /opt/zuse/harnesses/\n'
            'RUN chmod -R a+rX /opt/zuse/harnesses && chmod -R a-w /opt/zuse/harnesses\n'
            'ENV ZUSE_HARNESS_ROOT=/opt/zuse/harnesses ZUSE_HARNESS_CHANNEL=staging\n'
            'USER 10001:10001\n')
        (args.output / 'Dockerfile').write_text(dockerfile)
        files = {str(path.relative_to(args.output)): file_sha(path)
                 for path in sorted(args.output.rglob('*')) if path.is_file()}
        result = {'schema': 1, 'status': 'candidate-only', 'runtimeImage': args.runtime_image,
                  'harnessRelease': args.harness_release, 'runtimeVersion': manifest['runtimeVersion'],
                  'adapterRevision': manifest['adapterRevision'], 'stateFormat': manifest['stateFormat'],
                  'platform': manifest['platform'], 'harnesses': manifest['harnesses'], 'files': files}
        result['contextSha256'] = sha(canonical(result))
        write(args.output / 'candidate.json', result)
        return result


def bundle(args):
    if not DIGEST.fullmatch(args.runtime_sha256) or file_sha(args.runtime) != args.runtime_sha256:
        raise ValueError('verified_runtime_digest_required')
    with args.runtime.open('rb') as binary:
        if binary.read(4) != b'\x7fELF':
            raise ValueError('linux_native_runtime_required')
    if args.output.exists():
        raise ValueError('new_empty_bundle_directory_required')
    spec = importlib.util.spec_from_file_location('harness_release', Path(__file__).with_name('harness-release.py'))
    manager = importlib.util.module_from_spec(spec); spec.loader.exec_module(manager)
    with manager.locked(args.harness_root, read_only=True):
        source, manifest = manager.release(args.harness_root,args.harness_release,verify=True)
        if manifest['schema'] != 2 or not manifest['platform'].startswith('linux-'):
            raise ValueError('three_native_linux_harnesses_required')
        args.output.mkdir(mode=0o700,parents=True)
        files = {}
        for agent in AGENTS:
            with (source/agent).open('rb') as binary:
                if binary.read(4) != b'\x7fELF': raise ValueError('native_harness_required')
            files[agent] = source/agent
        if manifest['harnesses']['codex'].get('assets'):
            asset_archive=args.output/'codex-assets.tar'
            manager.archive_assets(source,manifest['harnesses']['codex']['assets'],asset_archive)
            files['codex-assets.tar']=asset_archive
        elif manifest['harnesses']['codex']['version']=='0.166.0':
            raise ValueError('complete_codex_distribution_required')
        files.update({'runtime':args.runtime,'harness-manifest.json':source/'manifest.json'})
        for name in ('harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py'):
            files[name] = Path(__file__).with_name(name)
        boat_boot=bool(getattr(args,'boat_post_hydration_bootstrap',False));fresh_boot = bool(getattr(args, 'fresh_memory_bootstrap', False)) or boat_boot
        if boat_boot and getattr(args,'fresh_memory_bootstrap',False):raise ValueError('ambiguous_bootstrap_mode')
        if fresh_boot:
            files['managed-base-fresh-boot.py'] = Path(__file__).with_name('managed-base-fresh-boot.py')
        identifier = 'zuse-base-' + sha(canonical({'runtime':args.runtime_sha256,'harness':args.harness_release,'files':{n:file_sha(p) for n,p in files.items()}}))[:32]
        plan = dict(managedBase=True,rustRuntimeSupervisorVersion=1,buildId=identifier,templateVersion=identifier,configurationDigest=identifier,
                    home='/home/zuse',cacheRoot='/home/zuse/.zuse-image/cache',statusDirectory='/opt/zuse/preparation/status',
                    secretsRoot='/opt/zuse/preparation/secrets',maxCacheBytes=1048576,repositories=[])
        write(args.output/'prepare-plan.json',plan)
        for name,path in files.items():
            if path!=args.output/name:shutil.copyfile(path,args.output/name)
            (args.output/name).chmod(0o555 if name in AGENTS or name=='runtime' or name.endswith('.sh') else 0o444)
        entries=[dict(path=p.name,sha256=file_sha(p),bytes=p.stat().st_size) for p in sorted(args.output.iterdir())]
        result=dict(schema=1,status='candidate-only',releaseId=identifier,harnessRelease=args.harness_release,
                    runtimeSha256=args.runtime_sha256,runtimeVersion=manifest['runtimeVersion'],platform=manifest['platform'],files=entries)
        if fresh_boot:
            result['managedBaseFreshBootVersion'] = 1
            if boat_boot:result['managedBaseFreshBootMode']='post-hydration-first-start-v1'
        write(args.output/'bundle.json',result)
        return result


def validate_release(release):
    if release.get('provider') not in PROVIDERS:
        raise ValueError('unknown_provider')
    for key in ('releaseId', 'artifactId', 'architecture', 'manifest', 'capabilities', 'harnesses'):
        if not release.get(key):
            raise ValueError('incomplete_provider_release')
    manifest = release['manifest']
    if not manifest.get('managedBase') or manifest.get('credentialsRemoved') is not True or manifest.get('repositories') != []:
        raise ValueError('clean_guest_managed_manifest_required')
    if {h.get('agent') for h in release['harnesses']} != set(AGENTS):
        raise ValueError('all_three_pinned_harnesses_required')
    if any(not DIGEST.fullmatch(h.get('sha256', '')) for h in release['harnesses']):
        raise ValueError('invalid_harness_digest')
    if release['capabilities'].get('id') != release['provider']:
        raise ValueError('provider_capability_mismatch')


def invoke(driver, request, provider, timeout):
    # Driver credentials remain environment-only and are never copied to reports.
    env = {key: value for key, value in os.environ.items()
           if key in ('PATH', 'SSL_CERT_FILE', 'SSL_CERT_DIR', KEYS[provider])
           or key.startswith('ZUSE_CERT_') or (provider == 'boxd' and key in ('BOXD_ORG', 'BOXD_BASE_URL'))}
    with tempfile.TemporaryFile() as input_file:
        input_file.write(canonical(request))
        input_file.seek(0)
        child = subprocess.Popen([str(driver)], stdin=input_file, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, env=env, start_new_session=True)
        output = bytearray()
        deadline = time.monotonic() + timeout
        try:
            with selectors.DefaultSelector() as streams:
                streams.register(child.stdout, selectors.EVENT_READ)
                streams.register(child.stderr, selectors.EVENT_READ)
                while streams.get_map():
                    remaining = deadline - time.monotonic()
                    if remaining <= 0:
                        raise ValueError('certification_driver_timeout')
                    for key, _ in streams.select(min(remaining, 0.2)):
                        block = os.read(key.fileobj.fileno(), 8192)
                        if not block:
                            streams.unregister(key.fileobj)
                        elif key.fileobj is child.stdout:
                            output.extend(block)
                            if len(output) > 64 * 1024:
                                raise ValueError('certification_driver_response_too_large')
                        # stderr is drained but never retained or reported.
            child.wait(timeout=max(0.001, deadline - time.monotonic()))
            if child.returncode:
                raise ValueError('certification_driver_failed')
            try:
                response = json.loads(output)
            except (ValueError, UnicodeError):
                raise ValueError('invalid_certification_driver_response') from None
        except (ValueError, subprocess.TimeoutExpired):
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait()
            raise ValueError('certification_driver_failed_or_timed_out') from None
        finally:
            child.stdout.close()
            child.stderr.close()
    if response.get('runId') != request['runId'] or response.get('provider') != provider:
        raise ValueError('certification_driver_identity_mismatch')
    return response


def certify(args):
    release = load(args.release)
    validate_release(release)
    # Report hash is excluded to avoid a circular binding. The artifact, guest
    # manifest, model catalogs and harness digests remain in the exact identity.
    candidate = {key: value for key, value in release.items() if key != 'certificationReportSha256'}
    binding = sha(canonical(candidate))
    run_id = 'zuse-cert-' + uuid.uuid4().hex
    request = {'schema': 1, 'runId': run_id, 'provider': release['provider'],
               'releaseSha256': binding, 'release': candidate, 'maxResources': 1,
               'maxLifetimeSeconds': 1800}
    report = {'schema': 1, 'status': 'incomplete', 'runId': run_id, 'provider': release['provider'],
              'releaseSha256': binding, 'driverSha256': sha(args.driver.read_bytes()),
              'cases': [], 'cleanupVerified': False}
    write(args.output, report)
    try:
        for agent in AGENTS:
            for case in CASES:
                started = time.monotonic()
                response = invoke(args.driver, dict(request, action='case', agent=agent, case=case),
                                  release['provider'], args.timeout)
                if response.get('agent') != agent or response.get('case') != case or response.get('releaseSha256') != binding:
                    raise ValueError('certification_case_identity_mismatch')
                passed = response.get('status') == 'passed'
                # Store bounded result codes/timings, never arbitrary tool output.
                report['cases'].append({'agent': agent, 'case': case, 'passed': passed,
                                        'elapsedMs': round((time.monotonic() - started) * 1000, 2)})
                write(args.output, report)
                if not passed:
                    raise ValueError('certification_case_not_passed')
        report['status'] = 'passed'
    except ValueError:
        report['status'] = 'failed'
        raise
    finally:
        try:
            cleanup = invoke(args.driver, dict(request, action='cleanup'), release['provider'], args.timeout)
            report['cleanupVerified'] = type(cleanup.get('remainingResources')) is int and cleanup['remainingResources'] == 0 and cleanup.get('status') == 'passed'
        except (ValueError, OSError):
            report['cleanupVerified'] = False
        if not report['cleanupVerified']:
            report['status'] = 'cleanup-required'
        write(args.output, report)
    if report['status'] != 'passed':
        raise ValueError('certification_cleanup_required')
    return report


def publication(args):
    release = load(args.release)
    validate_release(release)
    report = load(args.report)
    candidate = {key: value for key, value in release.items() if key != 'certificationReportSha256'}
    cases = report.get('cases', [])
    required = {(agent, case) for agent in AGENTS for case in CASES}
    if (report.get('status') != 'passed' or report.get('cleanupVerified') is not True
            or report.get('provider') != release['provider']
            or report.get('releaseSha256') != sha(canonical(candidate))
            or len(cases) != len(required)
            or {(row.get('agent'), row.get('case')) for row in cases} != required
            or any(row.get('passed') is not True for row in cases)):
        raise ValueError('complete_artifact_bound_live_report_required')
    candidate['certificationReportSha256'] = sha(args.report.read_bytes())
    write(args.output, candidate)
    return {'status': 'review-required', 'release': str(args.output),
            'registration': 'Use register-managed-base with explicit operator database role after review'}


def supported_e2b_sdk(value):
    """Accept stock 2.62 or the exact reviewed streaming upload scheduler."""
    integrity='sha512-RKYzGLgFLJa9xCct6Bh255UbERo/yaYqG/3sUqau60sZ38TFZSAt2M+eXGfKWo3yD31iu9iNVC44H0x9ppaoJw=='
    upstream='1c96b58fb06dd23f45d869a20800356b8712e33ff6c7b3bd04034e2fac112eaa'
    if value.get('sdkVersion')!='2.62.0':
        return False
    if value.get('sdkDerivation') is None:
        return (value.get('sdkIntegrity')==integrity and value.get('sdkModuleSha256',upstream)==upstream
                and not any(key in value for key in ('sdkDerivation','sdkUpstreamIntegrity','sdkUpstreamModuleSha256')))
    return (value.get('sdkDerivation')=='zuse-sequential-upload-v1'
            and value.get('sdkIntegrity') is None
            and value.get('sdkUpstreamIntegrity')==integrity
            and value.get('sdkUpstreamModuleSha256')==upstream
            and value.get('sdkModuleSha256')=='b69736a75bfdc0512683b55b38b971e4294f2382c2d2e820d819a8f3ad2242f9')


def portable_candidate_report(build, proof, bundle):
    """Validate portable build ownership; this is never live-agent certification."""
    provider = build.get('provider')
    if provider not in ('e2b', 'daytona'):
        return
    for value in (build, proof):
        if value.get('provider') != provider or value.get('status') != 'prepared-uncertified' or value.get('certification') is not False or value.get('registration') is not False or value.get('modelCredentials') is not False or value.get('retainArtifact') is not True:
            raise ValueError('portable_uncertified_retention_required')
        if value.get('runtimeSha256') != bundle.get('runtimeSha256') or value.get('harnessRelease') != bundle.get('harnessRelease') or not all(DIGEST.fullmatch(str(value.get(k, ''))) for k in ('bundleSha256','helperSha256','builderSha256')):
            raise ValueError('portable_installed_pin_binding_required')
        resident=value.get('residentRestore', {})
        if resident.get('status') != 'resident-ready' or resident.get('runtimeSha256') != bundle.get('runtimeSha256'):
            raise ValueError('portable_fresh_resident_required')
        gh=value.get('githubCliProof', {})
        if gh.get('status') != 'passed' or gh.get('required') is not True or gh.get('canonicalPath') != '/opt/zuse/bin/gh' or gh.get('ancestorsControlled') is not True or gh.get('profileMatches') is not True or gh.get('loginShellMatchesGlobal') is not True or gh.get('globalVersionMatchesReceipt') is not True:
            raise ValueError('portable_managed_github_cli_proof_required')
        if provider=='e2b':
            if not DIGEST.fullmatch(str(value.get('bootSha256',''))) or resident.get('bootSealVerified') is not True or resident.get('bootSha256')!=value.get('bootSha256'):
                raise ValueError('portable_managed_boot_seal_required')
            binding=value.get('providerArtifactBinding', {})
            if set(binding)!=set(('version','mode','artifactId','artifactVersion','artifactName','runtimeSha256','manifestSha256')) or binding.get('version')!=1 or binding.get('mode')!='private-single-build' or any(binding.get(k)!=expected for k,expected in (('artifactId',value.get('artifactId')),('artifactVersion',value.get('buildId')),('artifactName',value.get('name')),('runtimeSha256',bundle.get('runtimeSha256')),('manifestSha256',value.get('manifestSha256')))) or not DIGEST.fullmatch(str(binding.get('manifestSha256',''))):
                raise ValueError('portable_private_build_binding_required')
            if not supported_e2b_sdk(value) or value.get('artifactId')!=value.get('templateId') or value.get('providerArtifactVersion')!=value.get('buildId') or not value.get('buildId'):
                raise ValueError('portable_original_template_build_required')
        else:
            if value.get('sdkIntegrity')!='sha512-KxSKEOew52nUvi+oJIaTOEnDV1MQJ6j6JafmzguD6tKgBhwJM0k9C/jUGwSiksxdjzx5FFr1sSVTwhtd3uZE7Q==' or value.get('sdkVersion')!='0.227.0' or value.get('sandboxClass')!='container' or value.get('artifactId')!=value.get('snapshotId') or value.get('providerArtifactVersion')!=value.get('snapshotId') or not value.get('snapshotId') or value.get('memoryPreservationVerified') is not False or value.get('diskRetentionVerified') is not True or value.get('coldResidentRestore',{}).get('status')!='resident-ready' or value.get('coldResidentRestore',{}).get('runtimeSha256')!=bundle.get('runtimeSha256') or not DIGEST.fullmatch(str(value.get('dockerfileSha256',''))):
                raise ValueError('portable_persistent_container_snapshot_required')
    for key in ('operationId','artifactId','providerArtifactVersion','runtimeSha256','harnessRelease','bundleSha256','helperSha256','providerArtifactBinding','bootSha256','sdkVersion','sdkIntegrity','sdkDerivation','sdkUpstreamIntegrity','sdkUpstreamModuleSha256','sdkModuleSha256'):
        if build.get(key)!=proof.get(key):raise ValueError('portable_original_operation_binding_required')



def disabled_parent_hygiene(build, lineage, validator):
    """Validate a never-started parent without inventing a resident stop."""
    old = build.get('parentSupervisorHygiene', {})
    preflight = build.get('stoppedParentPreflight', {})
    image = old.get('parentImageProof', {})
    binding = build['cleanParent']
    pins = lineage['builder']['initialRuntimeBootstrap']
    expected = dict(version=1, parentMode=validator.DISABLED_PARENT_MODE,
                    parentArtifactId=binding['artifactId'],
                    parentRuntimeSha256=binding['parentRuntimeSha256'],
                    parentLineageSha256=binding['lineageSha256'],
                    runtimeSha256=build['runtimeSha256'],
                    harnessRelease=binding['harnessRelease'],
                    bootServiceSha256=pins['serviceSha256'],
                    reusedFiles=sorted(validator.SKIP), sourceClean=True,
                    completeCodexAssetsVerified=True,
                    noAuthorityPresent=True, noResidentPresent=True)
    return (validator.parent_mode(lineage) == validator.DISABLED_PARENT_MODE
            and old.get('version') == 1
            and old.get('alreadyDisabledParent') is True
            and old.get('legacyBuilder') is False
            and old.get('atomicStopAcknowledged') is False
            and old.get('startedOrStoppedProcess') is False
            and old.get('disabledBootServiceRetained') is True
            and old.get('enabledBootServiceRetained') is False
            and old.get('noAuthorityPresent') is True
            and old.get('noResidentPresent') is True
            and old.get('operationId') == build['runId']
            and old.get('parentMode') == validator.DISABLED_PARENT_MODE
            and old.get('parentLineageSha256') == binding['lineageSha256']
            and old.get('runtimeSha256') == binding['parentRuntimeSha256']
            and old.get('bootServiceSha256') == pins['serviceSha256']
            and image == dict(expected, status='quiescence-image-verified')
            and preflight == dict(expected, status='passed')
            and not any(k in old for k in ('residentPid', 'residentStartTicks', 'bootNonceSha256')))


def candidate_derivation(build, proof, bundle, directory):
    """Admit one retained clean-parent derivation, never arbitrary extra payloads."""
    has_helper = any(e.get('path') == 'managed-base-derive.py' for e in bundle['files'])
    if not has_helper and 'managedBaseDerivationVersion' not in bundle:
        return False
    if not has_helper or bundle.get('managedBaseDerivationVersion') != 1 or build.get('provider') != 'boat':
        raise ValueError('candidate_derivation_v1_required')
    path = directory / 'parent-lineage.json'
    if path.is_symlink() or not path.is_file():
        raise ValueError('candidate_derivation_lineage_required')
    lineage = load(path)
    binding = build.get('cleanParent', {})
    if (binding.get('lineageSha256') != file_sha(path)
            or build.get('bundleSha256') != file_sha(directory / 'bundle.json')
            or proof.get('sourceMachineAbsent') is not True
            or proof.get('runtimeSha256') != build.get('runtimeSha256')):
        raise ValueError('candidate_derivation_publication_binding_required')
    spec = importlib.util.spec_from_file_location('candidate_parent_binding', Path(__file__).with_name('managed-base-derive.py'))
    validator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validator)
    # This shared validator checks exact embedded parent report digests, clean
    # manifest, original artifact/VM absence and unchanged complete native assets.
    validator.parent_binding(lineage, bundle)
    parent = lineage['builder']
    if (binding.get('artifactId') != parent.get('artifactId')
            or binding.get('providerArtifactVersion') != parent.get('providerArtifactVersion')
            or binding.get('parentRuntimeSha256') != parent.get('runtimeSha256')
            or binding.get('parentBundleSha256') != lineage.get('parentBundleFileSha256')
            or binding.get('harnessRelease') != bundle.get('harnessRelease')
            or set(binding.get('reusedFiles', [])) != validator.SKIP):
        raise ValueError('candidate_derivation_parent_binding_required')
    plan = load(directory / 'prepare-plan.json')
    # The derived bundle names its payload closure independently; its immutable
    # installed deployment plan retains the original build identity.
    if (build.get('releaseId') != bundle.get('releaseId')
            or any(not isinstance(plan.get(k), str) or build['manifest'].get(k) != plan[k]
                   for k in ('buildId', 'templateVersion', 'configurationDigest'))):
        raise ValueError('candidate_derivation_installed_plan_binding_required')
    old = build.get('parentSupervisorHygiene', {})
    fresh = build.get('supervisorHygiene', {})
    if validator.parent_mode(lineage) == validator.DISABLED_PARENT_MODE:
        parent_clean = disabled_parent_hygiene(build, lineage, validator)
    else:
        parent_clean = (old.get('legacyBuilder') is True
                        and old.get('operationId') == build.get('runId')
                        and old.get('parentLineageSha256') == binding['lineageSha256']
                        and old.get('runtimeDataAbsent') is True
                        and old.get('originalResidentExited') is True
                        and old.get('supervisorLockReleased') is True
                        and build.get('stoppedParentPreflight', {}).get('status') == 'passed'
                        and 'alreadyDisabledParent' not in old)
    if (not parent_clean
            or fresh.get('legacyBuilder') is not False or fresh.get('atomicStopAcknowledged') is not True
            or fresh.get('operationId') != build.get('runId')
            or fresh.get('runtimeSha256') != bundle.get('runtimeSha256')
            or fresh.get('runtimeDataAbsent') is not True or fresh.get('originalResidentExited') is not True
            or fresh.get('supervisorLockReleased') is not True):
        raise ValueError('candidate_derivation_hygiene_required')
    return True


def candidate(args):
    """Package retained staging evidence; never labels it certified or registers it."""
    if args.output.exists():
        raise ValueError('new_candidate_file_required')
    build, proof, bundle = load(args.build_report), load(args.proof), load(args.bundle/'bundle.json')
    if build.get('status') != 'prepared-uncertified' or not build.get('cleanupVerified') or not build.get('artifactRetained'):
        raise ValueError('retained_prepared_artifact_required')
    if proof.get('status') not in ('prepared-uncertified','mechanism-verified-uncertified') or not proof.get('cleanupVerified') or not proof.get('artifactRetained'):
        raise ValueError('retained_candidate_proof_required')
    if proof.get('artifactId') != build.get('artifactId') or not build.get('artifactId'):
        raise ValueError('original_artifact_binding_required')
    if proof.get('status') == 'mechanism-verified-uncertified' and proof.get('candidateArtifactStatus') != 'uncertified-staging':
        raise ValueError('explicit_staging_retention_required')
    portable_candidate_report(build, proof, bundle)
    if build.get('provider') in ('e2b','daytona') and (build.get('bundleSha256')!=file_sha(args.bundle/'bundle.json') or proof.get('bundleSha256')!=file_sha(args.bundle/'bundle.json')):
        raise ValueError('portable_exact_bundle_file_required')
    manifest=build['manifest']
    if not manifest.get('managedBase') or not manifest.get('credentialsRemoved') or manifest.get('repositories') != [] or manifest.get('rustRuntimeSupervisorVersion') != 1:
        raise ValueError('clean_supervised_manifest_required')
    if bundle.get('schema') != 1 or bundle.get('status') != 'candidate-only' or bundle.get('runtimeSha256') != build.get('runtimeSha256'):
        raise ValueError('exact_runtime_bundle_binding_required')
    allowed={'runtime','claude','codex','grok','harness-manifest.json','harness-release.py','launch-runtime.sh','launch-recovery.sh','managed-base-install.sh','runtime-supervisor.py','runtime-repair.py','runtime-recreation.py','prepare-plan.json'}
    entries=bundle['files']
    names={e['path'] for e in entries}
    if 'codex-assets.tar' in names:allowed.add('codex-assets.tar')
    if bundle.get('managedBaseFreshBootVersion') == 1:allowed.add('managed-base-fresh-boot.py')
    elif 'managedBaseFreshBootVersion' in bundle:raise ValueError('candidate_fresh_boot_version_invalid')
    if bundle.get('managedBaseFreshBootMode') not in (None,'post-hydration-first-start-v1') or 'managedBaseFreshBootMode' in bundle and bundle.get('managedBaseFreshBootVersion')!=1:raise ValueError('candidate_fresh_boot_mode_invalid')
    legacy=allowed-{'runtime-repair.py','runtime-recreation.py'}
    derived = bundle.get('managedBaseDerivationVersion') == 1
    if len(entries)!=len(names) or names not in (allowed,legacy,allowed | {'managed-base-derive.py'} if derived else allowed):
        raise ValueError('bundle_allowlist_required')
    if names==legacy and (manifest.get('rustRuntimeRepairVersion') is not None or manifest.get('rustRecreationVersion') is not None):
        raise ValueError('extension_bundle_helpers_required')
    for entry in entries:
        path=args.bundle/entry['path']
        if path.is_symlink() or path.stat().st_size!=entry['bytes'] or file_sha(path)!=entry['sha256']:
            raise ValueError('bundle_integrity_failed')
    derived = candidate_derivation(build, proof, bundle, args.bundle)
    if not derived and bundle.get('releaseId') != manifest.get('buildId'):
        raise ValueError('exact_runtime_bundle_binding_required')
    harness=load(args.bundle/'harness-manifest.json')
    if sha(canonical(harness))!=bundle['harnessRelease'] or harness.get('schema')!=2:
        raise ValueError('exact_harness_release_required')
    if bool(harness['harnesses']['codex'].get('assets'))!=('codex-assets.tar' in names):raise ValueError('codex_distribution_bundle_required')
    capabilities,catalogs=load(args.capabilities),load(args.catalogs)
    provider=build['provider']
    if provider=='daytona' and capabilities.get('preservesMemory') is not False:
        raise ValueError('container_memory_preservation_unverified')
    if provider not in PROVIDERS or capabilities.get('id')!=provider:
        raise ValueError('provider_capability_binding_required')
    if not catalogs or not set(catalogs).issubset(AGENTS):
        raise ValueError('reviewed_candidate_catalog_subset_required')
    pins=[]
    for agent in AGENTS:
        pin=harness['harnesses'][agent]
        executable=next(e for e in entries if e['path']==agent)
        if pin['sha256']!=executable['sha256']:
            raise ValueError('installed_harness_digest_required')
        if agent not in catalogs:
            continue
        catalog=catalogs[agent]
        if catalog['id']!=('claude-code' if agent=='claude' else agent) or not catalog.get('models'):
            raise ValueError('reviewed_candidate_catalog_required')
        pins.append(dict(agent=agent,version=pin['version'],sha256=pin['sha256'],catalog=catalog))
    platform=bundle['platform']
    architecture=platform.removeprefix('linux-')
    if platform not in ('linux-x86_64','linux-aarch64'):
        raise ValueError('native_linux_architecture_required')
    value=dict(provider=provider,artifactId=build['artifactId'],architecture=architecture,
        capabilities=capabilities,manifest=manifest,harnesses=pins,harnessRelease=bundle['harnessRelease'],
        runtimeSha256=bundle['runtimeSha256'],provenanceReportSha256=file_sha(args.proof))
    if bundle.get('managedBaseFreshBootVersion') == 1:
        spec=build.get('initialRuntimeBootstrap',{})
        keys={'version','mode','runtimeSha256','supervisorSha256','serviceSha256','manifestSha256','helperSha256'}
        boat_mode=bundle.get('managedBaseFreshBootMode')=='post-hydration-first-start-v1'
        if (provider!=('boat' if boat_mode else 'boxd') or set(spec)!=keys or spec.get('version')!=1 or spec.get('mode')!=('post-hydration-first-start-v1' if boat_mode else 'stopped-memory-snapshot-v1')
                or any(not DIGEST.fullmatch(spec.get(k,'')) for k in keys-{'version','mode'})
                or spec['runtimeSha256']!=bundle['runtimeSha256']
                or spec['helperSha256']!=next(e['sha256'] for e in entries if e['path']=='managed-base-fresh-boot.py')
                or spec['supervisorSha256']!=next(e['sha256'] for e in entries if e['path']=='runtime-supervisor.py')
                or proof.get('initialRuntimeBootstrap')!=spec):
            raise ValueError('candidate_fresh_boot_pin_binding_required')
        observed=proof.get('initialRuntimeBootstrapReceipt',{});request=observed.get('request',{});receipt=observed.get('receipt',{})
        pins={k:spec[k] for k in keys-{'version','mode'}}
        if (set(request)!={'version','provider','purpose','operationId','workspaceId','generation','storageIncarnationId','sourceMachineId','imagePins'}
                or request.get('version')!=1 or type(request.get('generation')) is not int or request['generation']<1
                or any(not isinstance(request.get(k),str) or not re.fullmatch(r'[A-Za-z0-9_.:-]{1,180}',request[k]) for k in ('workspaceId','storageIncarnationId','operationId','sourceMachineId'))
                or request.get('provider')!=provider or request.get('purpose')!='publication-proof'
                or request.get('sourceMachineId')!=proof.get('restoreMachineId')
                or request.get('operationId')!=build.get('runId') or request.get('imagePins')!=pins
                or receipt.get('status')!='ready' or any(receipt.get(k)!=v for k,v in request.items())
                or receipt.get('requestSha256')!=sha(canonical(request))
                or receipt.get('resident',{}).get('ipcVerified') is not True
                or receipt.get('resident',{}).get('unbound') is not True
                or receipt.get('resident',{}).get('childAlive') is not False
                or receipt.get('resident',{}).get('recoveryAlive') is not False
                or type(receipt.get('resident',{}).get('residentPid')) is not int or receipt['resident']['residentPid']<=1
                or type(receipt.get('resident',{}).get('residentStartTicks')) is not int or receipt['resident']['residentStartTicks']<1
                or not DIGEST.fullmatch(receipt.get('resident',{}).get('bootNonceSha256',''))):
            raise ValueError('candidate_fresh_boot_actual_restore_required')
        if boat_mode:
            disabled=build.get('disabledBootCapture',{})
            if disabled.get('unitDisabled') is not True or disabled.get('operationId')!=build.get('runId') or disabled.get('runtimeSha256')!=bundle['runtimeSha256'] or disabled.get('helperSha256')!=spec['helperSha256'] or disabled.get('serviceSha256')!=spec['serviceSha256'] or proof.get('unitRemainsDisabled') is not True or proof.get('cleanColdRestartVerified') is not True or proof.get('taskRetentionVerified') is not False:raise ValueError('candidate_disabled_boot_capture_required')
        value['initialRuntimeBootstrap']=spec
    if provider=='e2b':value['providerArtifactBinding']=build['providerArtifactBinding']
    # Catalogue metadata is immutable independently of the clean machine image.
    value['releaseId']=manifest['buildId']+'-canary-'+sha(canonical(value))[:16]
    write(args.output,value)
    return dict(status='uncertified-staging-candidate',candidate=str(args.output),provider=provider,
        registration='stage-managed-base-candidate requires staging operator role and exact singleton owner pin; no verified registration')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    build = commands.add_parser('context')
    build.add_argument('--harness-root', required=True, type=Path)
    build.add_argument('--harness-release', required=True)
    build.add_argument('--runtime-image', required=True)
    build.add_argument('--output', required=True, type=Path)
    native = commands.add_parser('bundle')
    native.add_argument('--harness-root',required=True,type=Path)
    native.add_argument('--harness-release',required=True)
    native.add_argument('--runtime',required=True,type=Path)
    native.add_argument('--runtime-sha256',required=True)
    native.add_argument('--output',required=True,type=Path)
    native.add_argument('--fresh-memory-bootstrap',action='store_true')
    native.add_argument('--boat-post-hydration-bootstrap',action='store_true')
    canary = commands.add_parser('candidate')
    canary.add_argument('--build-report',required=True,type=Path)
    canary.add_argument('--proof',required=True,type=Path)
    canary.add_argument('--bundle',required=True,type=Path)
    canary.add_argument('--capabilities',required=True,type=Path)
    canary.add_argument('--catalogs',required=True,type=Path)
    canary.add_argument('--output',required=True,type=Path)
    check = commands.add_parser('certify')
    check.add_argument('--release', required=True, type=Path)
    check.add_argument('--driver', required=True, type=Path)
    check.add_argument('--timeout', type=int, default=300, choices=range(1, 901))
    check.add_argument('--output', required=True, type=Path)
    publish = commands.add_parser('publication')
    publish.add_argument('--release', required=True, type=Path)
    publish.add_argument('--report', required=True, type=Path)
    publish.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps({'context': context, 'bundle': bundle, 'certify': certify, 'publication': publication, 'candidate': candidate}[args.action](args)))
    except (ValueError, OSError, KeyError, TypeError):
        # Third-party output, credentials and local private paths are not diagnostics.
        parser.exit(1, 'Managed base release operation failed; inspect the private result report.\n')


if __name__ == '__main__':
    main()
