#!/usr/bin/env python3
"""Compare retrieved text with full fixture documents and check labeled fact coverage."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def compact(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':')).encode('utf-8')


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def normalized(text):
    return ' '.join(text.casefold().split())


def fact_coverage(facts, texts):
    """Facts are exact normalized strings in their labeled document, not semantic grades."""
    found = [f for f in facts if normalized(f['text']) in normalized(texts.get(f['path'], ''))]
    return {'covered': len(found), 'required': len(facts),
            'missing': [f for f in facts if f not in found], 'sufficient': len(found) == len(facts)}


def load_fixture(directory):
    notes = {}
    for path in sorted((directory / 'notes').rglob('*')):
        if path.is_symlink():
            raise ValueError('fixture symlinks are not supported')
        if path.is_file():
            if path.suffix != '.md':
                raise ValueError('fixture notes must be Markdown')
            notes[path.relative_to(directory / 'notes').as_posix()] = path.read_bytes().decode('utf-8')
    cases = json.loads((directory / 'cases.json').read_text(encoding='utf-8'))
    if not notes or not isinstance(cases, list) or not cases:
        raise ValueError('fixture must contain notes and a nonempty case array')
    seen = set()
    for case in cases:
        if not isinstance(case, dict) or not isinstance(case.get('id'), str) or not case['id'] or case['id'] in seen:
            raise ValueError('invalid/duplicate case id')
        seen.add(case['id'])
        if case.get('mode') not in ('search', 'context') or not isinstance(case.get('query'), str) or not case['query'].strip():
            raise ValueError('case requires mode and nonempty query')
        budget = case.get('max_tokens', 4000)
        if type(budget) is not int or not 512 <= budget <= 32768:
            raise ValueError('max_tokens must be 512..32768')
        if not isinstance(case.get('facts'), list):
            raise ValueError('facts must be an array')
        for fact in case['facts']:
            if not isinstance(fact, dict) or not isinstance(fact.get('path'), str) or not isinstance(fact.get('text'), str):
                raise ValueError('fact requires path and text')
            path = PurePosixPath(fact['path'])
            if path.is_absolute() or '..' in path.parts or '\\' in fact['path'] or fact['path'] not in notes:
                raise ValueError('unsafe or missing labeled path')
            if not normalized(fact['text']) or normalized(fact['text']) not in normalized(notes[fact['path']]):
                raise ValueError('labeled fact must occur in its full document')
    return cases, notes


def invoke(binary, args, cwd):
    process = subprocess.run([str(binary), *args], cwd=cwd, capture_output=True,
                             text=True, encoding='utf-8', timeout=120)
    if process.returncode:
        raise RuntimeError(f'CLI exited {process.returncode}: {process.stderr[:1500]}')
    return process.stdout


def compare(case, payload, explained, notes, hashes):
    if case['mode'] == 'search':
        clean = json.loads(json.dumps(explained))
        for hit in clean['hits']:
            if not isinstance(hit.pop('explanation', None), dict):
                raise ValueError('search explanation missing')
        entries, field = payload['hits'], 'body'
    else:
        clean = explained['context']
        entries, field = payload['entries'], 'excerpt'
        budget = case.get('max_tokens', 4000)
        accounting = explained['explanation']['budget']
        actual = len(compact(payload))
        if actual > budget or accounting['used_bytes'] != actual or payload['estimated_tokens'] != actual:
            raise ValueError('context byte accounting mismatch')
        if sum(accounting['entry_bytes_by_reason'].values()) + accounting['metadata_and_container_bytes'] != actual:
            raise ValueError('context byte groups do not reconcile')
    if clean != payload:
        raise ValueError('explain changed retrieval output')
    texts, full, identities = {}, {}, []
    for entry in entries:
        path = entry['path']
        if path not in notes or entry['content_hash'] != hashes[path] or not isinstance(entry.get(field), str):
            raise ValueError('unexpected path/hash/text from CLI')
        texts[path] = texts.get(path, '') + '\n' + entry[field]
        full[path] = notes[path]
        identities.append({key: entry.get(key) for key in ('source_id', 'path', 'content_hash', 'chunk_id', 'lines')})
    excerpt_bytes = sum(len(e[field].encode('utf-8')) for e in entries)
    full_bytes = sum(len(text.encode('utf-8')) for text in full.values())
    partial_facts = fact_coverage(case['facts'], texts)
    full_facts = fact_coverage(case['facts'], full)
    if not case['facts']:
        outcome = 'negative_pass' if not entries else 'negative_violation'
    elif partial_facts['sufficient']:
        outcome = 'sufficient_excerpt'
    elif full_facts['sufficient']:
        outcome = 'excerpt_loss'
    else:
        outcome = 'retrieval_miss'
    return {'id': case['id'], 'mode': case['mode'], 'query': case['query'],
            'outcome': outcome, 'identities': identities,
            'excerpt_text_bytes': excerpt_bytes, 'selected_full_document_text_bytes': full_bytes,
            'text_saving_fraction': 1 - excerpt_bytes / full_bytes if full_bytes else None,
            'payload_bytes': len(compact(payload)), 'explained_payload_bytes': len(compact(explained)),
            'explain_additional_payload_bytes': len(compact(explained)) - len(compact(payload)),
            'same_chunk_refetch_text_bytes': excerpt_bytes if case['mode'] == 'search' else None,
            'excerpt_fact_coverage': partial_facts, 'selected_full_document_fact_coverage': full_facts,
            'explain_parity': True}


def evaluate(binary, fixtures):
    binary, fixtures = binary.resolve(), fixtures.resolve()
    cases, notes = load_fixture(fixtures)
    hashes = {p: hashlib.sha256(text.encode('utf-8')).hexdigest() for p, text in notes.items()}
    inputs = [fixtures / 'cases.json', *sorted((fixtures / 'notes').rglob('*.md'))]
    report = {'schema_version': 1, 'binary_version': invoke(binary, ['--version'], ROOT).strip(),
              'binary_sha256': digest(binary),
              'fixture_sha256': {p.relative_to(fixtures).as_posix(): digest(p) for p in inputs},
              'method': 'Natural search top 1 or context at labeled budget; identical selected documents for excerpt/full comparison. Full text read from hash-verified isolated fixtures, not timed memory_get calls.',
              'limitations': ['UTF-8 text bytes, not model tokens or latency.',
                              'Exact normalized fact presence, not generated-answer correctness.',
                              'Small authored synthetic corpus; misses and excerpt losses are expected diagnostic outcomes.',
                              'Full documents deduplicated within each case; no cache across cases.',
                              'Search already returns body: refetch bytes model avoidable repeated text, excluding MCP envelopes.',
                              'Explain payload is measured separately and is outside the context budget.'],
              'cases': []}
    with tempfile.TemporaryDirectory(prefix='agent-memory-economy-') as temporary:
        work = Path(temporary)
        target = work / 'notes'
        shutil.copytree(fixtures / 'notes', target)
        config = work / 'owner.json'
        config.write_text(json.dumps({'version': 1, 'projects': [
            {'name': 'bench-economy', 'sources': [{'path': str(target), 'can_read': True, 'can_write': False}]}]}), encoding='utf-8')
        dbargs = ['--database', str(work / 'index.sqlite')]
        invoke(binary, [*dbargs, 'rebuild', '--config', str(config)], work)
        for case in cases:
            args = [*dbargs, case['mode'], '--project', 'bench-economy']
            if case['mode'] == 'search':
                args += ['--json', '--mode', 'natural', '--limit', '1']
            else:
                args += ['--max-tokens', str(case.get('max_tokens', 4000))]
            try:
                plain = json.loads(invoke(binary, [*args, '--', case['query']], work))
                explained = json.loads(invoke(binary, [*args, '--explain', '--', case['query']], work))
                report['cases'].append(compare(case, plain, explained, notes, hashes))
            except (RuntimeError, ValueError, KeyError, TypeError, OSError, subprocess.TimeoutExpired) as error:
                report['cases'].append({'id': case['id'], 'mode': case['mode'], 'error': str(error)})
    good = [r for r in report['cases'] if 'error' not in r]
    outcomes = ('sufficient_excerpt', 'excerpt_loss', 'retrieval_miss', 'negative_pass', 'negative_violation')
    report['summary'] = {'cases': len(cases), 'operational_errors': len(cases) - len(good),
                         **{key: sum(r['outcome'] == key for r in good) for key in outcomes},
                         'excerpt_text_bytes': sum(r['excerpt_text_bytes'] for r in good),
                         'selected_full_document_text_bytes': sum(r['selected_full_document_text_bytes'] for r in good)}
    return report


def markdown(report):
    lines = ['# Context economy benchmark', '', report['method'], '',
             '| Case | Outcome | Excerpt bytes | Full document bytes | Explain extra bytes |',
             '| --- | --- | ---: | ---: | ---: |']
    for row in report['cases']:
        values = [row['id'], row.get('outcome', 'error'), row.get('excerpt_text_bytes', ''),
                  row.get('selected_full_document_text_bytes', ''), row.get('explain_additional_payload_bytes', '')]
        lines.append('| ' + ' | '.join(str(v).replace('|', '\\|').replace('\n', ' ') for v in values) + ' |')
    lines += ['', '## Summary', '', '```json', json.dumps(report['summary'], indent=2), '```', '',
              '## Limits', '', *['- ' + s for s in report['limitations']], '']
    return '\n'.join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/debug/agent-memory')
    parser.add_argument('--fixtures', type=Path, default=ROOT / 'tests/context-economy')
    parser.add_argument('--output', type=Path, help='JSON report and companion .md')
    args = parser.parse_args(argv)
    try:
        if args.output and args.output.suffix != '.json':
            raise ValueError('--output must end in .json')
        report = evaluate(args.binary, args.fixtures)
        if args.output:
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
            args.output.with_suffix('.md').write_text(markdown(report), encoding='utf-8')
        print(json.dumps(report['summary'] if args.output else report, ensure_ascii=False, indent=2))
        return 2 if report['summary']['operational_errors'] else 0
    except (ValueError, OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f'benchmark failed: {error}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
