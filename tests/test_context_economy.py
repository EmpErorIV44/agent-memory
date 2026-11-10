import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'context_economy', Path(__file__).resolve().parents[1] / 'scripts/benchmark-context-economy.py')
bench = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bench)


class ContextEconomyTests(unittest.TestCase):
    def setUp(self):
        self.case = {'id': 'one', 'mode': 'search', 'query': 'policy',
                     'facts': [{'path': 'one.md', 'text': 'Keep history.'}]}
        self.notes = {'one.md': '# Policy\nKeep history.\n## Tail\nAlso keep journals.\n',
                      'wrong.md': '# Other\nUnrelated.\n'}
        import hashlib
        self.hashes = {p: hashlib.sha256(t.encode()).hexdigest() for p, t in self.notes.items()}

    def payload(self, path='one.md', body='Keep history.'):
        return {'hits': [{'path': path, 'content_hash': self.hashes[path], 'body': body}]}

    def compare(self, payload, explained=None):
        if explained is None:
            explained = copy.deepcopy(payload)
            for hit in explained['hits']:
                hit['explanation'] = {'mode': 'natural'}
        return bench.compare(self.case, payload, explained, self.notes, self.hashes)

    def test_distinguishes_sufficient_excerpt_loss_and_retrieval_miss(self):
        self.assertEqual(self.compare(self.payload())['outcome'], 'sufficient_excerpt')
        self.assertEqual(self.compare(self.payload(body='Policy only'))['outcome'], 'excerpt_loss')
        self.assertEqual(self.compare(self.payload('wrong.md'))['outcome'], 'retrieval_miss')
        self.assertEqual(self.compare({'hits': []})['outcome'], 'retrieval_miss')

    def test_unicode_bytes_hashes_and_fact_path_are_not_substituted(self):
        self.case['facts'] = [{'path': 'one.md', 'text': 'Keep history.'}]
        wrong = self.compare(self.payload('wrong.md', 'KEEP  HISTORY.'))
        self.assertEqual(wrong['outcome'], 'retrieval_miss')
        row = self.compare(self.payload(body='🐈 Привет'))
        self.assertEqual(row['excerpt_text_bytes'], len('🐈 Привет'.encode()))
        self.assertEqual(row['same_chunk_refetch_text_bytes'], len('🐈 Привет'.encode()))
        self.assertTrue(bench.fact_coverage(self.case['facts'], {'one.md': 'KEEP\nHISTORY.'})['sufficient'])

    def test_negative_and_invalid_explain_or_hash_are_explicit(self):
        self.case['facts'] = []
        self.assertEqual(self.compare({'hits': []})['outcome'], 'negative_pass')
        self.assertEqual(self.compare(self.payload())['outcome'], 'negative_violation')
        with self.assertRaises(ValueError):
            self.compare(self.payload(), self.payload())
        payload = self.payload()
        payload['hits'][0]['content_hash'] = 'forged'
        with self.assertRaises(ValueError):
            self.compare(payload)
        explained = self.payload(body='changed')
        explained['hits'][0]['explanation'] = {}
        with self.assertRaises(ValueError):
            self.compare(self.payload(), explained)

    def test_full_documents_deduplicated_within_case(self):
        payload = self.payload()
        payload['hits'].append(dict(payload['hits'][0]))
        row = self.compare(payload)
        self.assertEqual(row['selected_full_document_text_bytes'], len(self.notes['one.md'].encode()))
        self.assertEqual(row['excerpt_text_bytes'], 2 * len('Keep history.'.encode()))

    def test_context_budget_and_explain_parity_are_checked(self):
        self.case.update(mode='context', max_tokens=512)
        plain = {'entries': [], 'estimated_tokens': 0}
        while plain['estimated_tokens'] != len(bench.compact(plain)):
            plain['estimated_tokens'] = len(bench.compact(plain))
        size = len(bench.compact(plain))
        explained = {'context': plain, 'explanation': {'budget': {
            'used_bytes': size, 'entry_bytes_by_reason': {}, 'metadata_and_container_bytes': size}}}
        row = bench.compare(self.case, plain, explained, self.notes, self.hashes)
        self.assertTrue(row['explain_parity'])
        explained['explanation']['budget']['used_bytes'] += 1
        with self.assertRaises(ValueError):
            bench.compare(self.case, plain, explained, self.notes, self.hashes)

    def test_fixture_labels_validate_and_evaluation_is_isolated(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'notes').mkdir()
            note = root / 'notes/one.md'
            note.write_text(self.notes['one.md'], encoding='utf-8')
            cases = root / 'cases.json'
            cases.write_text(json.dumps([self.case]), encoding='utf-8')
            self.assertEqual(len(bench.load_fixture(root)[0]), 1)
            for bad in [dict(self.case, max_tokens=True), dict(self.case, mode='invalid'),
                        dict(self.case, facts=[{'path': '../one.md', 'text': 'Keep history.'}]),
                        dict(self.case, facts=[{'path': 'one.md', 'text': 'absent fact'}])]:
                cases.write_text(json.dumps([bad]), encoding='utf-8')
                with self.assertRaises(ValueError):
                    bench.load_fixture(root)
            cases.write_text(json.dumps([self.case]), encoding='utf-8')
            def invoke(binary, args, cwd):
                if args == ['--version']:
                    return 'test-version'
                if 'rebuild' in args:
                    config = json.loads((cwd / 'owner.json').read_text())
                    source = config['projects'][0]['sources'][0]
                    self.assertFalse(source['can_write'])
                    self.assertNotEqual(Path(source['path']), root / 'notes')
                    self.assertTrue(str(cwd) in str(args))
                    return '{}'
                value = self.payload()
                if '--explain' in args:
                    value['hits'][0]['explanation'] = {}
                return json.dumps(value)
            before = note.read_bytes()
            with patch.object(bench, 'invoke', side_effect=invoke):
                report = bench.evaluate(note, root)
            self.assertEqual(report['summary']['operational_errors'], 0)
            self.assertEqual(note.read_bytes(), before)
            self.assertIn('notes/one.md', report['fixture_sha256'])


if __name__ == '__main__':
    unittest.main()
