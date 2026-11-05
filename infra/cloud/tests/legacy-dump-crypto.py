import importlib.util
from pathlib import Path
import unittest
sp=importlib.util.spec_from_file_location('probe',Path(__file__).resolve().parents[1]/'legacy-dump-crypto.py')
m=importlib.util.module_from_spec(sp);sp.loader.exec_module(m)
class CryptoRows(unittest.TestCase):
 def test_copy_parser_retains_only_bound_account_and_ciphertext(self):
  text='-- ignored metadata\nCOPY public.api_cloud_github_users (account_id, login, name, email, sealed_credentials) FROM stdin;\nfixture-owner\tunused\tprivate name\tprivate@example.invalid\t{"version":1,"iv":"abc","ciphertext":"def"}\n\\.\n'
  rows=m.encrypted_rows(text)
  self.assertEqual(rows,[{'accountId':'fixture-owner','sealed':'{"version":1,"iv":"abc","ciphertext":"def"}'}])
  self.assertNotIn('private name',repr(rows));self.assertNotIn('example.invalid',repr(rows))
 def test_copy_escaping_and_cross_table_or_truncated_stream_rejected(self):
  self.assertEqual(m.unescape(r'a\tb\\c'), 'a\tb\\c')
  self.assertIsNone(m.unescape(r'\N'))
  with self.assertRaises(ValueError):m.encrypted_rows('COPY public.other (account_id) FROM stdin;\n')
  with self.assertRaises(ValueError):m.encrypted_rows('COPY public.api_cloud_github_users (account_id, sealed_credentials) FROM stdin;\nowner\tsealed\n')
if __name__=='__main__':unittest.main()
