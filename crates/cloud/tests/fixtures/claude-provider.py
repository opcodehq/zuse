#!/usr/bin/env python3
import os,sys,json,time
if '--version' in sys.argv:
 print('2.1.228');sys.exit(0)
assert os.environ.get('ANTHROPIC_API_KEY')=='fixture-provider-secret-never-persist'
assert 'CLAUDE_CODE_OAUTH_TOKEN' not in os.environ
assert 'ANTHROPIC_AUTH_TOKEN' not in os.environ
for line in sys.stdin:
 m=json.loads(line)
 if m.get('type')=='control_request':
  print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':m['request_id'],'response':{'models':[{'value':'fixture','displayName':'Fixture'}],'commands':[]}}}),flush=True)
 elif m.get('type')=='user':
  if '--system-prompt' not in sys.argv:
   with open(os.environ['ZUSE_TEST_PROVIDER_SEEN'],'a') as f:f.write('credential-only-in-child\n')
  print(json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'Brokered native provider reply'}}}),flush=True)
  if '--system-prompt' not in sys.argv: time.sleep(2)
  print(json.dumps({'type':'result','subtype':'success','result':'Brokered native provider reply','usage':{'input_tokens':1,'output_tokens':1},'session_id':'broker-native-session'}),flush=True)
  break
