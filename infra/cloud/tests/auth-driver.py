#!/usr/bin/env python3
import base64
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPT=Path(__file__).resolve().parents[1]/'auth-driver.py'
spec=importlib.util.spec_from_file_location('auth_driver',SCRIPT)
driver=importlib.util.module_from_spec(spec);spec.loader.exec_module(driver)


class AuthorityDriverTest(unittest.TestCase):
    def test_native_device_login_creates_a_private_explicit_profile_before_exec(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);home=root/'profile';home.mkdir(mode=0o700);fake=root/'codex'
            fake.write_text("""#!/usr/bin/env python3
import os,json,base64,time,stat
from pathlib import Path
home=Path(os.environ['CODEX_HOME'])
assert home.is_dir() and stat.S_IMODE(home.stat().st_mode)==0o700
claims=base64.urlsafe_b64encode(json.dumps({'exp':int(time.time())+3600}).encode()).decode().rstrip('=')
(home/'auth.json').write_text(json.dumps({'tokens':{'access_token':'header.'+claims+'.signature','refresh_token':'synthetic-refresh','account_id':'fixture-account'}}))
print('https://auth.openai.com/codex/device\\nDevice code: ABCD-1234',flush=True)
time.sleep(0.05)
""");fake.chmod(0o700)
            with patch.dict(os.environ,{'ZUSE_AUTH_CODEX_BINARY':str(fake)}), patch.object(driver,'emit'):
                credential=driver.device_login('codex',home,driver.isolated_environment(home),3)
            self.assertEqual(credential['providerAccountId'],'fixture-account')
            for leaf in ('.codex','.grok','.claude','.config'):
                self.assertEqual((home/leaf).stat().st_mode & 0o777,0o700)

    def test_refresh_preserves_rotation_even_when_later_account_read_fails(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);home=root/'profile';home.mkdir();fake=root/'codex'
            fake.write_text('''#!/usr/bin/env python3
import sys,json,os,base64,time
from pathlib import Path
for line in sys.stdin:
 request=json.loads(line)
 if request.get('id')==1: print(json.dumps({'id':1,'result':{}}),flush=True)
 elif request.get('id')==2:
  path=Path(os.environ['CODEX_HOME'])/'auth.json';record=json.loads(path.read_text())
  claims=base64.urlsafe_b64encode(json.dumps({'exp':int(time.time())+3600}).encode()).decode().rstrip('=')
  record['tokens']['access_token']='header.'+claims+'.signature';record['tokens']['refresh_token']='rotated-test-refresh'
  path.write_text(json.dumps(record));print(json.dumps({'id':2,'error':{'message':'private vendor diagnostic'}}),flush=True)
''');fake.chmod(0o700)
            original={'refreshToken':'old-test-refresh','nativeAuth':{'tokens':{'access_token':'old','refresh_token':'old-test-refresh','account_id':'test-account'}}}
            env=driver.isolated_environment(home)
            with patch.dict(os.environ,{'ZUSE_AUTH_CODEX_BINARY':str(fake)}):
                refreshed=driver.refresh_native('codex',original,home,env)
            self.assertEqual(refreshed['refreshToken'],'rotated-test-refresh')
            self.assertEqual(refreshed['providerAccountId'],'test-account')

    def test_device_challenge_is_parsed_without_exposing_cli_diagnostics(self):
        challenge=driver.parse_device_output('codex','private CLI diagnostic\nOpen https://auth.openai.com/codex/device\nEnter this one-time code: ABCD-1234\n')
        self.assertEqual(challenge,{'event':'device-code','verificationUrl':'https://auth.openai.com/codex/device','verificationCode':'ABCD-1234'})
        self.assertNotIn('private',json.dumps(challenge))

    def test_challenges_do_not_allow_foreign_hosts_or_credential_urls(self):
        for url in ['https://attacker.example/device','https://auth.openai.com.attacker.example/device','https://user:secret@auth.openai.com/device','https://auth.openai.com/device#secret']:
            with self.assertRaises(driver.AuthFailure):
                driver.parse_device_output('codex',f'Open {url}\nDevice code: ABCD-1234')
        self.assertIsNone(driver.parse_device_output('grok','No challenge yet'))

    def test_pinned_cli_standalone_codes_ansi_and_help_links_do_not_hide_the_challenge(self):
        output='\x1b]0;private title\x07\x1b[32mhttps://auth.x.ai/device\x1b[0m\r\n\nABCD-1234\nHelp: https://docs.x.ai/build\n'
        challenge=driver.parse_device_output('grok',output)
        self.assertEqual(challenge['verificationUrl'],'https://auth.x.ai/device')
        self.assertEqual(challenge['verificationCode'],'ABCD-1234')

    def test_verification_cannot_inherit_another_accounts_credentials_or_proxy(self):
        with patch.dict(os.environ,{'OPENAI_API_KEY':'wrong','ANTHROPIC_API_KEY':'wrong','XAI_API_KEY':'wrong','HTTPS_PROXY':'https://wrong','CODEX_HOME':'/wrong','BOXD_API_KEY':'infrastructure'}):
            env=driver.isolated_environment(Path('/private/profile'))
        for name in ['OPENAI_API_KEY','ANTHROPIC_API_KEY','XAI_API_KEY','HTTPS_PROXY','BOXD_API_KEY']:
            self.assertNotIn(name,env)
        self.assertEqual(env['CODEX_HOME'],'/private/profile/.codex')

    def test_native_login_requires_renewable_credentials_and_actual_expiry(self):
        with tempfile.TemporaryDirectory() as directory:
            home=Path(directory);(home/'.codex').mkdir()
            claims=base64.urlsafe_b64encode(json.dumps({'exp':int(time.time())+3600,'email':'test@example.test'}).encode()).decode().rstrip('=')
            value={'tokens':{'access_token':f'header.{claims}.signature','refresh_token':'renewable-test-value','account_id':'provider-account'}}
            path=home/'.codex/auth.json';path.write_text(json.dumps(value))
            verified=driver.read_auth(home,'codex')
            self.assertEqual(verified['providerAccountId'],'provider-account')
            self.assertEqual(verified['nativeAuth'],value)
            value['tokens'].pop('refresh_token');path.write_text(json.dumps(value))
            with self.assertRaises(driver.AuthFailure):driver.read_auth(home,'codex')

    def test_grok_native_profile_requires_a_real_expiry_and_unambiguous_account(self):
        from datetime import datetime, timezone
        with tempfile.TemporaryDirectory() as directory:
            home=Path(directory);(home/'.grok').mkdir()
            token={'key':'synthetic-access','refresh_token':'synthetic-refresh','user_id':'selected-user',
                   'expires_at':datetime.fromtimestamp(time.time()+3600,timezone.utc).isoformat()}
            path=home/'.grok/auth.json'
            path.write_text(json.dumps({'https://auth.x.ai::account':token}))
            verified=driver.read_auth(home,'grok')
            self.assertEqual(verified['providerAccountId'],'selected-user')
            self.assertEqual(verified['refreshToken'],'synthetic-refresh')
            token.pop('expires_at');token['create_time']=datetime.now(timezone.utc).isoformat()
            path.write_text(json.dumps({'https://auth.x.ai::account':token}))
            with self.assertRaises(driver.AuthFailure):driver.read_auth(home,'grok')

    def test_invalid_requests_return_static_errors_without_tracebacks_or_secret_echo(self):
        result=subprocess.run([sys.executable,str(SCRIPT)],input=json.dumps({'agent':'invalid-private-agent','method':'device-code','secret':'private-test-secret'}),capture_output=True,text=True)
        self.assertNotEqual(result.returncode,0)
        self.assertEqual(json.loads(result.stdout),{'event':'error','code':'auth_method_unsupported'})
        self.assertEqual(result.stderr,'')
        self.assertNotIn('private-test-secret',result.stdout)

    def test_real_device_driver_uses_isolated_cli_and_cleans_up_credential_files(self):
        with tempfile.TemporaryDirectory() as directory:
            fake=Path(directory)/'codex'
            fake.write_text('''#!/usr/bin/env python3
import os,json,base64,time
from pathlib import Path
home=Path(os.environ['CODEX_HOME']);home.mkdir(exist_ok=True)
claims=base64.urlsafe_b64encode(json.dumps({'exp':int(time.time())+3600}).encode()).decode().rstrip('=')
(home/'auth.json').write_text(json.dumps({'tokens':{'access_token':'header.'+claims+'.signature','refresh_token':'private-test-refresh','account_id':'test-account'}}))
print('Open https://auth.openai.com/codex/device',flush=True)
print('One-time code: ABCD-1234',flush=True)
''');fake.chmod(0o700)
            result=subprocess.run([sys.executable,str(SCRIPT)],input=json.dumps({'agent':'codex','method':'device-code','timeoutSeconds':5}),env=dict(os.environ,ZUSE_AUTH_CODEX_BINARY=str(fake)),capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stdout)
            events=[json.loads(line) for line in result.stdout.splitlines()]
            self.assertEqual(events[0]['event'],'device-code')
            self.assertEqual(events[-1]['event'],'verified')
            self.assertEqual(events[-1]['credential']['providerAccountId'],'test-account')
            self.assertNotIn('private-test-refresh',json.dumps(events[:-1]))
            # The driver manages a temporary HOME, never the fixture directory.
            self.assertFalse((Path(directory)/'.codex').exists())


if __name__=='__main__':unittest.main()
