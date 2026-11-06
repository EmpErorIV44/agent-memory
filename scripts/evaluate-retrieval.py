#!/usr/bin/env python3
"""Measure frozen retrieval labels using a disposable CLI database (stdlib only)."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load_suite(directory):
    cases = json.loads((directory / 'cases.json').read_text(encoding='utf-8'))
    if not isinstance(cases, list) or not cases:
        raise ValueError(f'{directory}: cases must be a nonempty array')
    seen = set()
    for case in cases:
        if not isinstance(case, dict):
            raise ValueError('each case must be an object')
        for key in ('id', 'mode', 'query', 'category', 'rationale'):
            if not isinstance(case.get(key), str) or not case[key].strip():
                raise ValueError(f'missing/non-string {key}')
        if case['id'] in seen or case['mode'] not in ('search', 'context'):
            raise ValueError(f'duplicate id or invalid mode: {case["id"]}')
        seen.add(case['id'])
        for key in ('relevant', 'forbidden'):
            paths = case.get(key)
            if not isinstance(paths, list) or any(not isinstance(p, str) for p in paths):
                raise ValueError(f'{case["id"]}: {key} must be a path array')
            if len(paths) != len(set(paths)):
                raise ValueError(f'{case["id"]}: duplicate {key} path')
            for path in paths:
                pure = PurePosixPath(path)
                if pure.is_absolute() or '..' in pure.parts or '\\' in path or not path.endswith('.md'):
                    raise ValueError(f'unsafe note path: {path}')
                if not (directory / 'notes' / path).is_file():
                    raise ValueError(f'missing labeled note: {path}')
        if set(case['relevant']) & set(case['forbidden']):
            raise ValueError('relevant and forbidden overlap')
        if 'literal' in case and not isinstance(case['literal'], bool):
            raise ValueError('literal must be boolean')
        budget = case.get('max_tokens', 4000)
        if isinstance(budget, bool) or not isinstance(budget, int) or not 512 <= budget <= 32768:
            raise ValueError('max_tokens must be 512..32768')
    return cases


def score(case, paths):
    paths = list(dict.fromkeys(paths))  # Rank documents, not repeated chunks.
    relevant = set(case['relevant'])
    found = relevant.intersection(paths)
    recall = lambda selected: len(relevant.intersection(selected)) / len(relevant) if relevant else None
    result = {
        'returned_paths': paths,
        'recall_at_5': recall(paths[:5]),
        'recall_at_10': recall(paths[:10]),
        'recall': recall(paths),
        'judged_precision': len(found) / len(paths) if paths else None,
        'reciprocal_rank': next((1 / i for i, p in enumerate(paths, 1) if p in relevant), 0) if relevant else None,
        'missing': sorted(relevant - set(paths)),
        'forbidden_returned': sorted(set(case['forbidden']).intersection(paths)),
    }
    # Empty-relevance cases are negative tests: any return is a miss even when
    # no individually forbidden document was specified.
    result['negative_violation'] = not relevant and bool(paths)
    result['quality_pass'] = not result['forbidden_returned'] and not result['negative_violation'] and (
        not relevant or (result['recall_at_10'] == 1 if case['mode'] == 'search' else result['recall'] == 1)
    )
    return result


def invoke(binary, args, cwd):
    try:
        process = subprocess.run([str(binary), *args], cwd=cwd, text=True,
                                 encoding='utf-8', capture_output=True, timeout=120)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise RuntimeError(f'CLI launch failed: {error}') from error
    if process.returncode:
        raise RuntimeError(f'CLI exited {process.returncode}: {process.stderr.strip()[:2000]}')
    return process.stdout


def evaluate(binary, fixtures):
    binary = binary.resolve()
    suites = sorted(p.parent for p in fixtures.glob('*/cases.json'))
    if not suites:
        raise ValueError(f'no suites under {fixtures}')
    loaded = [(suite, load_suite(suite)) for suite in suites]
    version = invoke(binary, ['--version'], ROOT).strip()
    inputs = set()
    for suite, _ in loaded:
        inputs.add(suite / 'cases.json')
        if (suite / 'README.md').is_file():
            inputs.add(suite / 'README.md')
        inputs.update(p for p in (suite / 'notes').rglob('*') if p.is_file())
    hashes = {str(p.relative_to(fixtures)): digest(p) for p in sorted(inputs)}
    report = {'binary_version': version, 'binary_sha256': digest(binary),
              'fixture_sha256': hashes, 'suite_count': len(loaded),
              'note_count': sum(len(list((suite / 'notes').rglob('*.md'))) for suite, _ in loaded),
              'method': 'document deduplication; search top 50 chunks then document ranks; context full returned set',
              'warning': 'Synthetic, author-judged labels; judged_precision treats unlabeled returns as irrelevant and is not proof of irrelevance. Quality misses are not automatically product defects.',
              'cases': []}
    with tempfile.TemporaryDirectory(prefix='agent-memory-retrieval-') as temporary:
        work = Path(temporary)
        projects = []
        for suite, _ in loaded:
            target = work / suite.name / 'notes'
            shutil.copytree(suite / 'notes', target)
            projects.append({'name': 'bench-' + suite.name, 'sources': [
                {'path': str(target), 'can_read': True, 'can_write': False}]})
        config = work / 'owner.json'
        config.write_text(json.dumps({'version': 1, 'projects': projects}), encoding='utf-8')
        dbargs = ['--database', str(work / 'index.sqlite')]
        invoke(binary, [*dbargs, 'rebuild', '--config', str(config)], work)
        for suite, cases in loaded:
            project = 'bench-' + suite.name
            note_hashes = {p.relative_to(suite / 'notes').as_posix(): digest(p)
                           for p in (suite / 'notes').rglob('*.md')}
            for case in cases:
                row = {'suite': suite.name, **case}
                try:
                    if case['mode'] == 'search':
                        args = ['search', '--project', project, '--json', '--limit', '50']
                        for key in ('kind', 'status'):
                            if key in case:
                                args.extend(['--' + key, case[key]])
                        if case.get('literal'):
                            args.append('--literal')
                        args.extend(['--', case['query']])
                        payload = json.loads(invoke(binary, [*dbargs, *args], work))
                        entries = payload['hits']
                    else:
                        budget = case.get('max_tokens', 4000)
                        args = ['context', '--project', project, '--max-tokens', str(budget), '--', case['query']]
                        payload = json.loads(invoke(binary, [*dbargs, *args], work))
                        entries = payload['entries']
                        row['context_diagnostics'] = {key: payload.get(key) for key in (
                            'max_tokens', 'estimated_tokens', 'omitted_count', 'omitted_accepted_constraints', 'budget_method', 'warnings')}
                        row['context_diagnostics']['truncated_excerpts'] = sum(bool(e.get('excerpt_truncated')) for e in entries)
                        row['context_diagnostics']['reasons'] = [e.get('reason') for e in entries]
                        actual_bytes = len(json.dumps(payload, ensure_ascii=False, separators=(',', ':')).encode('utf-8'))
                        row['context_diagnostics']['actual_compact_json_bytes'] = actual_bytes
                        if not isinstance(payload.get('estimated_tokens'), int) or payload['estimated_tokens'] > budget or actual_bytes > budget:
                            raise ValueError('context budget diagnostic missing or exceeded')
                    if not isinstance(entries, list) or any(not isinstance(e.get('path'), str) for e in entries):
                        raise ValueError('invalid CLI result entries')
                    for entry in entries:
                        path = entry['path']
                        if path not in note_hashes or entry.get('content_hash') != note_hashes[path]:
                            raise ValueError(f'returned note identity outside expected suite or mismatched hash: {path}')
                    row.update(score(case, [e['path'] for e in entries]))
                except (RuntimeError, ValueError, KeyError, TypeError, AttributeError) as error:
                    row['error'] = str(error)
                    row['quality_pass'] = False
                report['cases'].append(row)
    report['summary'] = summarize(report['cases'])
    report['by_suite'] = {suite.name: summarize([r for r in report['cases'] if r['suite'] == suite.name])
                          for suite, _ in loaded}
    return report


def summarize(rows):
    groups = {}
    for mode in ('search', 'context'):
        selected = [r for r in rows if r['mode'] == mode]
        ok = [r for r in selected if 'error' not in r]
        def mean(key):
            values = [r[key] for r in ok if r.get(key) is not None]
            return sum(values) / len(values) if values else None
        groups[mode] = {'cases': len(selected), 'errors': len(selected) - len(ok),
                        'quality_passes': sum(r['quality_pass'] for r in ok),
                        'mean_recall_at_5': mean('recall_at_5'), 'mean_recall_at_10': mean('recall_at_10'),
                        'mean_recall': mean('recall'), 'mean_reciprocal_rank': mean('reciprocal_rank'),
                        'mean_judged_precision': mean('judged_precision'),
                        'forbidden_violations': sum(bool(r['forbidden_returned']) for r in ok),
                        'negative_violations': sum(r['negative_violation'] for r in ok)}
    return groups


def markdown(report):
    def cell(value):
        return str(value).replace('|', '\\|').replace('\n', ' ')
    lines = ['# Synthetic retrieval evaluation', '', report['binary_version'], '', report['warning'], '',
             '| Suite | Case | Mode | Recall@5 | Recall@10 | Recall | Forbidden | Error |',
             '| --- | --- | --- | --- | --- | --- | --- | --- |']
    for row in report['cases']:
        values = [row['suite'], row['id'], row['mode'], row.get('recall_at_5'), row.get('recall_at_10'),
                  row.get('recall'), ', '.join(row.get('forbidden_returned', [])), row.get('error', '')]
        lines.append('| ' + ' | '.join(cell(v) for v in values) + ' |')
    lines.extend(['', '## Aggregate metrics', '', '```json', json.dumps(report['summary'], indent=2), '```', '',
                  'Full rankings, labels, context diagnostics, and SHA256 fingerprints are in the companion JSON report.', ''])
    return '\n'.join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/agent-memory')
    parser.add_argument('--fixtures', type=Path, default=ROOT / 'tests/retrieval')
    parser.add_argument('--output', type=Path, help='JSON file; companion Markdown uses the .md suffix')
    parser.add_argument('--fail-on-miss', action='store_true')
    args = parser.parse_args(argv)
    try:
        report = evaluate(args.binary, args.fixtures.resolve())
        if args.output:
            if args.output.suffix != '.json':
                raise ValueError('--output must end in .json')
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
            args.output.with_suffix('.md').write_text(markdown(report), encoding='utf-8')
        print(json.dumps(report['summary'] if args.output else report, ensure_ascii=False, indent=2))
        if any('error' in r for r in report['cases']):
            return 2
        if args.fail_on_miss and any(not r['quality_pass'] for r in report['cases']):
            return 1
        return 0
    except (ValueError, OSError, RuntimeError) as error:
        print(f'evaluation failed: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
