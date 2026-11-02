import contextlib
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location('localization_check',Path(__file__).with_name('check-localization.py'))
check=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(check)

class LocalizationReviewTests(unittest.TestCase):
    def setUp(self):
        self.directory=tempfile.TemporaryDirectory()
        self.root=Path(self.directory.name)
        self.previous=check.ROOT
        check.ROOT=self.root
        for locale in ['en','de']:
            (self.root/'locales'/locale).mkdir(parents=True)
        (self.root/'locales/en/common.ftl').write_text('-brand = Zuse\ncommon-save = Save { -brand }\ncommon-cancel = Cancel\n')
        (self.root/'locales/de/common.ftl').write_text('-brand = Zuse\ncommon-save = Speichern { -brand }\ncommon-cancel = Abbrechen\n')
        self.run_check('--update')
    def tearDown(self):
        check.ROOT=self.previous
        self.directory.cleanup()
    def run_check(self,*arguments):
        with patch('sys.argv',['check-localization.py',*arguments]),contextlib.redirect_stdout(io.StringIO()):check.main()
    def reviewed(self):
        state=json.loads((self.root/'review.json').read_text())
        state['reviews']={'de':{'common-save':{'status':'reviewed','sourceHash':check.hashes()['common-save'],'translationHash':check.hashes('de')['common-save'],'reviewer':'Native reviewer','evidence':'screenshots/save.png'}}}
        (self.root/'review.json').write_text(json.dumps(state))
        return state
    def test_changed_english_invalidates_only_affected_messages(self):
        self.reviewed()
        path=self.root/'locales/en/common.ftl'
        path.write_text(path.read_text().replace('Cancel','Cancel operation'))
        self.run_check('--update')
        reviews=json.loads((self.root/'review.json').read_text())['reviews']['de']
        self.assertEqual(reviews['common-save']['status'],'reviewed')
        self.assertEqual(reviews['common-cancel']['status'],'needs-review')
    def test_changed_translation_invalidates_its_review(self):
        self.reviewed()
        path=self.root/'locales/de/common.ftl'
        path.write_text(path.read_text().replace('Speichern','Sichern'))
        with self.assertRaises(SystemExit):self.run_check()
        before=path.read_bytes()
        self.run_check('--update')
        self.assertEqual(path.read_bytes(),before)
        self.assertEqual(json.loads((self.root/'review.json').read_text())['reviews']['de']['common-save']['status'],'needs-review')
    def test_term_changes_invalidate_dependent_messages(self):
        self.reviewed()
        path=self.root/'locales/en/common.ftl'
        path.write_text(path.read_text().replace('-brand = Zuse','-brand = Zuse Native'))
        self.run_check('--update')
        self.assertEqual(json.loads((self.root/'review.json').read_text())['reviews']['de']['common-save']['status'],'needs-review')
    def test_update_never_promotes_drafts_or_rewrites_catalogs(self):
        before={path:path.read_bytes() for path in (self.root/'locales').rglob('*.ftl')}
        self.run_check('--update')
        self.assertEqual(before,{path:path.read_bytes() for path in before})
        self.assertEqual(json.loads((self.root/'review.json').read_text())['reviews'],{})
    def test_review_needs_identity_and_screen_evidence(self):
        state=self.reviewed();del state['reviews']['de']['common-save']['evidence']
        (self.root/'review.json').write_text(json.dumps(state))
        with self.assertRaises(SystemExit):self.run_check()

if __name__=='__main__':unittest.main()
