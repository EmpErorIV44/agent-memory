import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'evaluate_retrieval', Path(__file__).resolve().parents[1] / 'scripts/evaluate-retrieval.py')
evaluation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(evaluation)


class RetrievalEvaluationTests(unittest.TestCase):
    def case(self, **changes):
        return dict(id='example', mode='search', query='cache', relevant=['target.md'],
                    forbidden=['old.md'], category='exact', rationale='current cache policy', **changes)

    def test_document_rank_deduplicates_chunks_before_cutoffs(self):
        case = self.case()
        result = evaluation.score(case, ['a.md'] * 20 + ['target.md', 'old.md'])
        self.assertEqual(result['reciprocal_rank'], .5)
        self.assertEqual(result['recall_at_5'], 1)
        self.assertEqual(result['judged_precision'], 1 / 3)
        self.assertEqual(result['forbidden_returned'], ['old.md'])
        self.assertFalse(result['quality_pass'])

    def test_top_ten_miss_differs_from_full_recall(self):
        result = evaluation.score(self.case(), [f'{i}.md' for i in range(10)] + ['target.md'])
        self.assertEqual(result['recall_at_10'], 0)
        self.assertEqual(result['recall'], 1)
        self.assertFalse(result['quality_pass'])
        case = self.case()
        case['mode'] = 'context'
        self.assertTrue(evaluation.score(case, [f'{i}.md' for i in range(10)] + ['target.md'])['quality_pass'])

    def test_negative_case_does_not_claim_perfect_recall(self):
        case = self.case()
        case['relevant'] = []
        empty = evaluation.score(case, [])
        self.assertIsNone(empty['recall'])
        self.assertIsNone(empty['reciprocal_rank'])
        self.assertTrue(empty['quality_pass'])
        found = evaluation.score(case, ['unlabeled.md'])
        self.assertTrue(found['negative_violation'])
        self.assertFalse(found['quality_pass'])

    def test_schema_rejects_missing_paths_overlap_and_traversal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'notes').mkdir()
            (root / 'notes/target.md').write_text('# Policy')
            (root / 'notes/old.md').write_text('# Old')
            good = self.case()
            (root / 'cases.json').write_text(json.dumps([good]))
            self.assertEqual(evaluation.load_suite(root), [good])
            for bad in [dict(good, relevant=['../target.md']), dict(good, forbidden=['target.md']),
                        dict(good, relevant=['absent.md']), dict(good, max_tokens=True),
                        dict(good, literal='true')]:
                (root / 'cases.json').write_text(json.dumps([bad]))
                with self.subTest(bad=bad), self.assertRaises(ValueError):
                    evaluation.load_suite(root)

    def test_cli_nonzero_and_invalid_output_are_operational_errors(self):
        process = type('Process', (), {'returncode': 7, 'stderr': 'unknown option', 'stdout': ''})()
        with patch.object(evaluation.subprocess, 'run', return_value=process):
            with self.assertRaisesRegex(RuntimeError, 'CLI exited 7: unknown option'):
                evaluation.invoke(Path('/fake'), ['--version'], Path('/tmp'))

    def test_evaluation_uses_isolated_readonly_copies_and_preserves_labels(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            suite = root / 'fixtures/en'
            (suite / 'notes').mkdir(parents=True)
            (suite / 'notes/target.md').write_text('# Current')
            (suite / 'notes/old.md').write_text('# Historical')
            search = self.case()
            search['query'] = '--punctuated'
            context = dict(search, id='context', mode='context', max_tokens=512)
            (suite / 'cases.json').write_text(json.dumps([search, context]))
            binary = root / 'binary'
            binary.write_bytes(b'fake executable for hash')
            seen = []
            def fake_invoke(selected_binary, args, cwd):
                seen.append(args)
                self.assertEqual(selected_binary, binary)
                if args == ['--version']:
                    return 'agent-memory fake'
                database = Path(args[1])
                self.assertTrue(database.is_relative_to(cwd))
                self.assertNotEqual(cwd, root)
                if 'rebuild' in args:
                    config = json.loads(Path(args[-1]).read_text())
                    source = config['projects'][0]['sources'][0]
                    self.assertFalse(source['can_write'])
                    self.assertTrue(source['can_read'])
                    copied = Path(source['path'])
                    self.assertNotEqual(copied, suite / 'notes')
                    self.assertEqual((copied / 'target.md').read_text(), '# Current')
                    return ''
                self.assertEqual(args[-2:], ['--', '--punctuated'])
                if 'search' in args:
                    hit = {'path': 'target.md', 'content_hash': evaluation.digest(suite / 'notes/target.md')}
                    return json.dumps({'hits': [hit, hit]})
                return json.dumps({'entries': [{'path': 'target.md', 'content_hash': evaluation.digest(suite / 'notes/target.md'), 'reason': 'task_match',
                                                'excerpt_truncated': True}],
                                   'estimated_tokens': 480, 'max_tokens': 512, 'omitted_count': 1})
            with patch.object(evaluation, 'invoke', side_effect=fake_invoke):
                result = evaluation.evaluate(binary, root / 'fixtures')
            self.assertEqual(len(seen), 4)
            self.assertEqual(result['summary']['search']['quality_passes'], 1)
            self.assertEqual(result['cases'][0]['returned_paths'], ['target.md'])
            self.assertEqual(result['cases'][1]['context_diagnostics']['truncated_excerpts'], 1)
            self.assertEqual((suite / 'notes/target.md').read_text(), '# Current')
            self.assertIn('en/cases.json', result['fixture_sha256'])
            (root / 'fixtures/baseline.json').write_text('{}')
            (root / 'fixtures/baseline.md').write_text('# Report')
            with patch.object(evaluation, 'invoke', side_effect=fake_invoke):
                rerun = evaluation.evaluate(binary, root / 'fixtures')
            self.assertEqual(result['fixture_sha256'], rerun['fixture_sha256'])
            seen.clear()
            with patch.object(evaluation, 'invoke', side_effect=fake_invoke):
                natural = evaluation.evaluate(binary, root / 'fixtures', 'natural')
            self.assertEqual(natural['search_mode'], 'natural')
            self.assertEqual(natural['fixture_sha256'], result['fixture_sha256'])
            searches = [args for args in seen if 'search' in args]
            self.assertEqual(searches[0][searches[0].index('--mode') + 1], 'natural')
            search['literal'] = True
            (suite / 'cases.json').write_text(json.dumps([search, context]))
            seen.clear()
            with patch.object(evaluation, 'invoke', side_effect=fake_invoke):
                evaluation.evaluate(binary, root / 'fixtures', 'natural')
            searches = [args for args in seen if 'search' in args]
            self.assertIn('--literal', searches[0])
            self.assertNotIn('--mode', searches[0])
            search.pop('literal')
            (suite / 'cases.json').write_text(json.dumps([search, context]))

            self.assertEqual(result['note_count'], 2)
            self.assertEqual(result['suite_count'], 1)
            original_fake = fake_invoke
            for failure in ('wrong_hash', 'oversized_context'):
                def bad_invoke(selected_binary, args, cwd):
                    text = original_fake(selected_binary, args, cwd)
                    if 'search' not in args and 'context' not in args:
                        return text
                    payload = json.loads(text)
                    if failure == 'wrong_hash':
                        entries = payload.get('hits', payload.get('entries'))
                        entries[0]['content_hash'] = 'another-suite-hash'
                    elif 'context' in args:
                        payload['entries'][0]['excerpt'] = 'я' * 600
                        payload['estimated_tokens'] = 480
                    return json.dumps(payload)
                with patch.object(evaluation, 'invoke', side_effect=bad_invoke):
                    invalid = evaluation.evaluate(binary, root / 'fixtures')
                errors = [r['error'] for r in invalid['cases'] if 'error' in r]
                self.assertTrue(errors)
                self.assertTrue(any('mismatched hash' in e if failure == 'wrong_hash' else 'budget' in e for e in errors))

    def test_exit_status_distinguishes_quality_from_operations(self):
        row = dict(self.case(), quality_pass=False, error='invalid JSON', suite='en')
        for rows, strict, expected in [([row], False, 2),
                                       ([dict(row, error=None)], False, 2),
                                       ([{'quality_pass': False}], False, 0),
                                       ([{'quality_pass': False}], True, 1)]:
            with patch.object(evaluation, 'evaluate', return_value={'cases': rows, 'summary': {}}), \
                    patch('builtins.print'):
                self.assertEqual(evaluation.main(['--fail-on-miss'] if strict else []), expected)


if __name__ == '__main__':
    unittest.main()
