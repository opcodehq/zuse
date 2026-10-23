#!/usr/bin/env python3
import os,sys,json,time,base64
from pathlib import Path
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
  reply='Brokered native provider reply'
  content=m.get('message',{}).get('content',[])
  blocks=content if isinstance(content,list) else [{'type':'text','text':content}]
  text='\n'.join(b.get('text','') for b in blocks if b.get('type')=='text')
  if '--system-prompt' not in sys.argv and os.environ.get('ZUSE_TEST_PROVIDER_IMAGE'):
   assert any(b.get('type')=='image' and base64.b64decode(b['source']['data'])==Path(os.environ['ZUSE_TEST_PROVIDER_IMAGE']).read_bytes() for b in blocks)

  if '--system-prompt' not in sys.argv and 'cloud-attachment.txt' in text:
   paths=[line[2:].strip() for line in text.splitlines() if line.startswith('- /')]
   note=next(Path(p) for p in paths if p.endswith('cloud-attachment.txt'))
   png=next(Path(p) for p in paths if p.endswith('cloud-attachment.png'))
   assert note.read_bytes()==b'cloud attachment bytes\n'
   assert any(b.get('type')=='image' and base64.b64decode(b['source']['data'])==png.read_bytes() for b in blocks)
   reply='Cloud attachment verified'
  print(json.dumps({'type':'stream_event' ,'event':{'type':'content_block_delta','delta':{'type':'text_delta','text':reply}}}),flush=True)
  if '--system-prompt' not in sys.argv: time.sleep(2)
  print(json.dumps({'type':'result','subtype':'success','result':reply,'usage':{'input_tokens':1,'output_tokens':1},'session_id':'broker-native-session'}),flush=True)
  break
