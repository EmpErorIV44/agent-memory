#!/usr/bin/env python3
"""Compare explicit natural coverage thresholds without changing frozen labels."""
import argparse
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('retrieval', ROOT / 'scripts/evaluate-retrieval.py')
retrieval = importlib.util.module_from_spec(spec)
spec.loader.exec_module(retrieval)


def run(binary, output):
    output.mkdir(parents=True, exist_ok=True)
    parity = {}
    binary_hash = None

    def checked_evaluate(fixtures, mode, threshold=None):
        nonlocal binary_hash
        before = retrieval.digest(binary)
        report = retrieval.evaluate(binary, fixtures, mode, threshold)
        after = retrieval.digest(binary)
        if binary_hash is None:
            binary_hash = before
        if not before == after == report['binary_sha256'] == binary_hash:
            raise RuntimeError('binary changed during coverage benchmark; rerun after builds finish')
        return report

    for mode in ['fts', 'natural']:
        current = checked_evaluate(ROOT / 'tests/retrieval', mode)
        frozen = json.loads((ROOT / f'tests/retrieval/search-{mode}.json').read_text(encoding='utf-8'))
        parity[mode] = {key: current[key] == frozen[key]
                        for key in ['fixture_sha256', 'cases', 'summary']}
        (output / f'unfiltered-{mode}.json').write_text(json.dumps(current, indent=2) + '\n', encoding='utf-8')
    (output / 'parity.json').write_text(json.dumps(parity, indent=2) + '\n', encoding='utf-8')
    if not all(all(checks.values()) for checks in parity.values()):
        raise RuntimeError(f'unfiltered baseline changed: {parity}')
    rows = []
    for corpus, fixtures in [('baseline', ROOT / 'tests/retrieval'),
                             ('noise', ROOT / 'tests/retrieval-noise')]:
        for threshold in [None, 0.0, 0.5, 0.6, 1.0]:
            report = checked_evaluate(fixtures, 'natural', threshold)
            report['min_coverage'] = threshold
            label = 'none' if threshold is None else str(threshold)
            (output / f'{corpus}-{label}.json').write_text(
                json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
            cases = [r for r in report['cases'] if r['mode'] == 'search']
            if any('error' in r for r in report['cases']):
                raise RuntimeError(f'operational error in {corpus}-{label}')
            rows.append({'corpus': corpus, 'min_coverage': threshold,
                         'passed': sum(r['quality_pass'] for r in cases),
                         'cases': len(cases), 'summary': report['summary']})
    (output / 'summary.json').write_text(json.dumps(rows, indent=2) + '\n', encoding='utf-8')
    lines = ['# Search coverage experiment', '',
             'Frozen FTS/natural fixture hashes, ordered case results and summaries match.', '',
             '| Corpus | Threshold | Passed | Recall@5 | Judged precision |',
             '| --- | --- | --- | --- | --- |']
    for row in rows:
        search = row['summary']['search']
        lines.append(f"| {row['corpus']} | {row['min_coverage']} | {row['passed']}/{row['cases']} | "
                     f"{search['mean_recall_at_5']:.3f} | {search['mean_judged_precision']:.3f} |")
    lines += ['', 'Synthetic labels are incomplete; judged precision is diagnostic.',
              'Coverage removes weak lexical matches but cannot reject complete lexical collisions.']
    (output / 'summary.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    print('\n'.join(lines))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/agent-memory')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    run(args.binary, args.output)
