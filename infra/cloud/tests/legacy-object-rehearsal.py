#!/usr/bin/env python3
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest

spec=importlib.util.spec_from_file_location('objects',Path(__file__).resolve().parents[1]/'legacy-object-rehearsal.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)

class Missing(Exception):
    response={'Error':{'Code':'NoSuchKey'},'ResponseMetadata':{'HTTPStatusCode':404}}

class Store:
    def __init__(self, source=False, lost_put=False, corrupt=False, oversized=False):
        self.objects={};self.writes=[];self.source=source;self.lost_put=lost_put
        self.corrupt=corrupt;self.oversized=oversized
    def seed(self,key,body): self.objects[key]=(body,{'ContentType':'application/octet-stream','Metadata':{'sealed':'yes'}})
    def head_object(self,Bucket,Key):
        if Key not in self.objects:raise Missing()
        body,meta=self.objects[Key]
        return dict(ContentLength=len(body),ETag='"'+hashlib.md5(body).hexdigest()+'"',**meta)
    def get_object(self,Bucket,Key,**kw):
        head=self.head_object(Bucket,Key)
        if kw.get('IfMatch',head['ETag'])!=head['ETag']:raise AssertionError('missing exact read fence')
        body=self.objects[Key][0]
        if self.corrupt:body=bytes([body[0]^1])+body[1:]
        if self.oversized:body+=b'extra'
        return dict(head,Body=io.BytesIO(body))
    def put_object(self,Bucket,Key,Body,ContentLength,IfNoneMatch,**meta):
        assert not self.source and IfNoneMatch=='*' and Key not in self.objects
        body=Body.read();assert len(body)==ContentLength
        self.objects[Key]=(body,meta);self.writes.append(('put',Key))
        if self.lost_put:raise TimeoutError()
    def delete_object(self,Bucket,Key):
        assert not self.source
        self.writes.append(('delete',Key));self.objects.pop(Key,None)

class ObjectRetention(unittest.TestCase):
    def run_copy(self,src,dst,objects=None):
        key='workspaces/w/sessions/s/generations/1/e/1.json'
        if objects is None:objects=[dict(key=key,kind='transcript-checkpoint',expectedBytes=6,expectedSha256=hashlib.sha256(b'sealed').hexdigest())]
        with tempfile.TemporaryDirectory() as d:
            path=Path(d)/'operation.json'
            result=m.rehearse(src,dst,'zuse-cloud-transcripts','new-staging',objects,path)
            journal=json.loads(path.read_text());self.assertEqual(path.stat().st_mode&0o777,0o600)
            self.assertEqual(journal['phase'],'cleaned')
            self.assertFalse(dst.objects);self.assertFalse(src.writes)
            return result,journal
    def source(self):
        src=Store(source=True);src.seed('workspaces/w/sessions/s/generations/1/e/1.json',b'sealed');return src
    def test_actual_bytes_metadata_sha_and_source_unchanged(self):
        result,journal=self.run_copy(self.source(),Store())
        self.assertIsNone(result['failure']);self.assertEqual(result['sampledObjects'],1)
        self.assertEqual(result['sampledBytes'],6);self.assertTrue(journal['entries'][0]['verified'])
        self.assertNotIn('sourceKey',json.dumps(result))
    def test_lost_put_response_still_cleaned(self):
        result,journal=self.run_copy(self.source(),Store(lost_put=True))
        self.assertEqual(result['failure'],'object_operation_failed');self.assertEqual(len(journal['entries']),1)
    def test_changed_copied_payload_rejected_and_cleaned(self):
        result,_=self.run_copy(self.source(),Store(corrupt=True))
        self.assertEqual(result['failure'],'copied_bytes_or_metadata_mismatch')
    def test_declared_source_hash_mismatch_never_puts(self):
        src=self.source();src.objects[next(iter(src.objects))]=(b'change',src.objects[next(iter(src.objects))][1])
        dst=Store();result,_=self.run_copy(src,dst)
        self.assertEqual(result['failure'],'source_reference_hash_mismatch');self.assertFalse(dst.writes)
    def test_count_and_byte_bounds(self):
        src=Store(source=True)
        for i in range(8):src.seed('workspaces/'+str(i),b'x')
        result,journal=self.run_copy(src,Store(),[dict(key='workspaces/'+str(i),kind='archive-chunk') for i in range(8)])
        self.assertEqual(result['sampledObjects'],5);self.assertEqual(len(journal['entries']),5)
        old=m.MAX_BYTES
        try:
            m.MAX_BYTES=5
            result,journal=self.run_copy(self.source(),Store())
            self.assertEqual(result['sampledObjects'],0);self.assertFalse(journal['entries'])
        finally:m.MAX_BYTES=old
    def test_stream_limit_and_truncated_source_prevent_put(self):
        src=self.source();src.oversized=True
        result,journal=self.run_copy(src,Store())
        self.assertEqual(result['failure'],'source_changed');self.assertFalse(journal['entries'])
    def test_copy_decoder_preserves_escaped_keys(self):
        rows=m.copy_rows('COPY public.api_cloud_workspaces (workspace_id, request_config) FROM stdin;\nw\\t1\t{"x":"a\\\\b"}\n\\.\n','api_cloud_workspaces')
        self.assertEqual(rows[0]['workspace_id'],'w\t1')
    def test_cleanup_scope_cannot_touch_source(self):
        dst=Store()
        with self.assertRaisesRegex(ValueError,'cleanup_scope_invalid'):
            m.cleanup(dst,'new-staging','migration-rehearsal/x/',[{'destinationKey':'workspaces/source'}])
        self.assertFalse(dst.writes)
    def test_bucket_inventory_is_metadata_only_and_reports_truncation(self):
        class Listing:
            def __init__(self):self.calls=0
            def list_objects_v2(self,**args):
                self.calls+=1
                return {'Contents':[{'Key':'workspaces/w/api-assets/asset_x.json','Size':4,'ETag':'opaque'}],
                        'IsTruncated':True,'NextContinuationToken':str(self.calls)}
        source=Listing()
        with tempfile.TemporaryDirectory() as d:
            r=m.bucket_inventory(source,'zuse-cloud-transcripts',Path(d)/'list.json',max_objects=1)
            self.assertFalse(r['completeBucketListing']);self.assertEqual(r['payloadsRead'],0)
            self.assertEqual(r['kinds'],{'attachment':1});self.assertEqual(source.calls,1)

if __name__=='__main__':unittest.main()
