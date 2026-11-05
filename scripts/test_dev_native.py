import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name('dev-native.py')
spec = importlib.util.spec_from_file_location('native_dev', SCRIPT)
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


class WatcherTest(unittest.TestCase):
    def test_docs_and_tests_do_not_trigger_native_rebuilds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root/'crates/ui/src/lib.rs'
            source.parent.mkdir(parents=True)
            source.write_text('fn render() {}')
            documentation = root/'crates/ui/README.md'
            documentation.write_text('docs')
            test = root/'crates/ui/tests/ui.rs'
            test.parent.mkdir()
            test.write_text('test')
            with patch.object(native, 'ROOT', root):
                before = native.snapshot()
                documentation.write_text('updated docs')
                test.write_text('updated tests')
                self.assertEqual(native.snapshot(), before,
                                 'non-runtime changes must not rebuild/relaunch the app')
                source.write_text('fn render() { changed(); }')
                self.assertNotEqual(native.snapshot(), before)



class DependencyAndReloadTest(unittest.TestCase):
    def test_only_runtime_and_build_dependencies_are_watched(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            def package(name, folder, dependencies=()):
                return {'name':name,'manifest_path':str(root/folder/'Cargo.toml'),'dependencies':list(dependencies)}
            def dep(folder, kind=None):
                return {'path':str(root/folder),'kind':kind}
            metadata={'packages':[
                package('zuse','apps/desktop',[dep('crates/ui'),dep('crates/engine'),dep('crates/build-helper','build'),dep('crates/test-helper','dev')]),
                package('zuse-ui','crates/ui',[dep('crates/protocol')]),
                package('zuse-engine','crates/engine',[dep('crates/protocol')]),
                package('zuse-protocol','crates/protocol'),
                package('build-helper','crates/build-helper'),
                package('test-helper','crates/test-helper'),
                package('cloud-api','apps/cloud'),
            ]}
            with patch.object(native,'ROOT',root):
                selected=set(native.package_roots(metadata))
            self.assertEqual(selected,{(root/folder).resolve() for folder in ['apps/desktop','crates/ui','crates/engine','crates/protocol','crates/build-helper']})

    def test_sync_timestamp_touches_are_ignored_but_asset_content_and_deletion_are_detected(self):
        import os
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            asset=root/'crates/ui/assets/prompt.md';asset.parent.mkdir(parents=True);asset.write_text('hello')
            with patch.object(native,'ROOT',root):
                watcher=native.SourceWatcher([root/'crates/ui'])
                before=watcher.scan()
                info=asset.stat();os.utime(asset,ns=(info.st_atime_ns,info.st_mtime_ns+1_000_000))
                self.assertEqual(watcher.scan(),before)
                asset.write_text('world')
                self.assertNotEqual(watcher.scan(),before)
                asset.unlink()
                self.assertNotIn(asset,watcher.scan())

    def test_fast_build_keeps_dependency_debug_setting_and_full_debug_is_available(self):
        command=native.cargo_build_command(environment={})
        self.assertIn('profile.dev.package."zuse".debug="line-tables-only"',command)
        self.assertFalse(any('package."*"' in item or item.startswith('profile.dev.debug=') for item in command))
        self.assertIn('profile.dev.package."zuse".debug=0',native.cargo_build_command(environment={'CARGO_PROFILE_DEV_DEBUG':'0'}))
        self.assertNotIn('--config',native.cargo_build_command(full_debug=True,environment={}))

    def test_noop_cargo_build_keeps_the_existing_ui_process(self):
        import json
        from unittest.mock import MagicMock
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/'target/debug/zuse';binary.parent.mkdir(parents=True);binary.write_text('compiled')
            source=root/'apps/desktop/src/main.rs'
            metadata={'target_directory':str(root/'target'),'packages':[{'name':'zuse','manifest_path':str(root/'apps/desktop/Cargo.toml'),'dependencies':[]}]}
            engine,ui=MagicMock(),MagicMock()
            engine.poll.return_value=None;ui.poll.return_value=None
            watcher=MagicMock();watcher.scan.side_effect=[{source:b'a'},{source:b'b'},{source:b'b'},{source:b'b'}]
            # Initial watch tick, save coalescing, then Ctrl+C.
            with patch.object(native,'ROOT',root), patch.object(native,'metadata',return_value=metadata), \
                 patch.object(native,'SourceWatcher',return_value=watcher), patch.object(native,'build',return_value=True), \
                 patch.object(native.subprocess,'Popen',side_effect=[engine,ui]) as launch, \
                 patch.object(native.socket,'socket'),patch.object(native.socket,'create_connection'), \
                 patch.object(native.signal,'signal'),patch.object(native.time,'sleep',side_effect=[None,None,KeyboardInterrupt]), \
                 patch('sys.argv',['dev-native.py','--data-dir',str(root/'profile')]):
                self.assertEqual(native.main(),0)
            self.assertEqual(launch.call_count,2,'a fresh executable must not restart the UI')


class CargoCacheTest(unittest.TestCase):
    def test_desktop_build_script_does_not_watch_missing_windows_files_on_other_platforms(self):
        import os, shutil, subprocess
        if not shutil.which('rustc'):
            self.skipTest('Rust compiler required for desktop build script check')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # Compile the real build script with a resource-library stub; the
            # Windows branch must not execute on either supported dev platform.
            stub = '''mod embed_resource {
                pub const NONE: u32 = 0;
                pub struct Output;
                pub fn compile_for(_: &str, _: &[&str], _: u32) -> Output { Output }
                impl Output { pub fn manifest_required(self) -> Result<(), String> { Ok(()) } }
            }\n'''
            script = root/'build.rs'
            script.write_text(stub + (SCRIPT.parent.parent/'apps/desktop/build.rs').read_text())
            binary = root/'build-script'
            subprocess.run(['rustc',str(script),'-o',str(binary)],env=dict(os.environ,CARGO_MANIFEST_DIR=str(root)),check=True,capture_output=True)
            for platform in ['macos','linux']:
                output = subprocess.check_output([str(binary)],env=dict(os.environ,CARGO_CFG_TARGET_OS=platform),text=True)
                self.assertNotIn('dist/windows/',output,'missing foreign-platform resources must not force every build to relink')

    def test_real_icon_build_script_stays_fresh_with_public_style_selector(self):
        import json, os, shutil, subprocess
        if not shutil.which('cargo'):
            self.skipTest('Cargo required for icon build freshness check')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)/'crates/icons'
            (root/'src').mkdir(parents=True)
            (Path(directory)/'scripts').mkdir()
            (Path(directory)/'scripts/prepare-icons.mjs').write_text('// fixture input\n')
            (root/'assets/free').mkdir(parents=True)
            (root/'Cargo.toml').write_text('[package]\nname="native-icon-fixture"\nversion="0.1.0"\nedition="2024"\n[workspace]\n')
            (root/'src/lib.rs').write_text('include!(concat!(env!("OUT_DIR"), "/icons.rs"));\n')
            (root/'mapping.tsv').write_text('test\tunused\n')
            (root/'assets/free/test.svg').write_text('<svg/>')
            shutil.copy2(SCRIPT.parent.parent/'crates/icons/build.rs', root/'build.rs')
            env = native.build_environment(dict(os.environ, HUGEICONS_LICENSE_KEY='', ZUSE_ICON_STYLE='free', CARGO_TARGET_DIR=str(root/'target')))
            self.assertEqual(native.build_environment({'HUGEICONS_LICENSE_KEY':'private'})['ZUSE_ICON_STYLE'], 'solid')
            self.assertEqual(native.build_environment({})['ZUSE_ICON_STYLE'], 'free')
            command = ['cargo', 'build', '--offline', '--message-format=json']
            subprocess.run(command, cwd=root, env=env, check=True, capture_output=True)
            result = subprocess.run(command, cwd=root, env=env, check=True, capture_output=True, text=True)
            artifacts = [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]
            self.assertTrue(all(item['fresh'] for item in artifacts if item.get('reason')=='compiler-artifact'),
                            'the real icon build script must not rebuild unchanged inputs')

    def test_lighter_workspace_debug_information_reuses_compiled_dependency(self):
        import json,os,shutil,subprocess
        if not shutil.which('cargo'):
            self.skipTest('Cargo required for artifact reuse check')
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);project=root/'app';dependency=root/'external';build_dependency=root/'build-tool'
            for folder in (project,dependency,build_dependency):
                (folder/'src').mkdir(parents=True)
            (build_dependency/'Cargo.toml').write_text('[package]\nname="native-build-helper"\nversion="0.1.0"\nedition="2024"\n[workspace]\n')
            (build_dependency/'src/lib.rs').write_text('pub fn build() {}\n')
            (dependency/'Cargo.toml').write_text('[package]\nname="native-dev-dependency"\nversion="0.1.0"\nedition="2024"\n[workspace]\n[build-dependencies]\nnative-build-helper={path="../build-tool"}\n')
            (dependency/'build.rs').write_text('fn main() { native_build_helper::build(); println!("cargo:rerun-if-changed=build.rs"); }\n')
            (dependency/'src/lib.rs').write_text('pub fn value() -> u64 { 7 }\n')
            (project/'Cargo.toml').write_text('[package]\nname="zuse"\nversion="0.1.0"\nedition="2024"\n[dependencies]\nnative-dev-dependency={path="../external"}\n[profile.dev.package."*"]\nopt-level=2\n')
            (project/'src/main.rs').write_text('fn main() { println!("{}",native_dev_dependency::value()); }\n')
            env={key:value for key,value in os.environ.items() if not key.startswith('CARGO_PROFILE_') and key not in {'CARGO_TARGET_DIR','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_INCREMENTAL'}}
            env['CARGO_TARGET_DIR']=str(root/'target')
            subprocess.run(['cargo','generate-lockfile','--offline'],cwd=project,env=env,check=True,capture_output=True)
            base=subprocess.run(['cargo','build','--locked','--offline','--message-format=json'],cwd=project,env=env,check=True,capture_output=True,text=True)
            fast=subprocess.run(native.cargo_build_command(environment=env)+['--offline','--message-format=json'],cwd=project,env=env,check=True,capture_output=True,text=True)
            artifacts=[json.loads(line) for line in fast.stdout.splitlines() if line.startswith('{')]
            dependency_artifact=next(item for item in artifacts if item.get('reason')=='compiler-artifact' and item['target']['name']=='native_dev_dependency')
            app_artifact=next(item for item in artifacts if item.get('reason')=='compiler-artifact' and item['target']['name']=='zuse')
            self.assertTrue(dependency_artifact['fresh'], 'the first-party debug setting must preserve the dependency cache')
            self.assertTrue(all(item['fresh'] for item in artifacts if item.get('reason')=='compiler-artifact' and item['target']['name']!='zuse'),
                            'third-party build scripts and their dependencies must also stay fresh')
            self.assertFalse(app_artifact['fresh'])
            again=subprocess.run(native.cargo_build_command(environment=env)+['--offline','--message-format=json'],cwd=project,env=env,check=True,capture_output=True,text=True)
            self.assertTrue(all(item['fresh'] for item in (json.loads(line) for line in again.stdout.splitlines() if line.startswith('{')) if item.get('reason')=='compiler-artifact'))


if __name__ == "__main__":
    unittest.main()
