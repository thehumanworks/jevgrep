#!/usr/bin/env python3
"""Opt-in live evaluation on committed synthetic fixtures; never a CI quality gate."""
import argparse
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent


def score(cases, reports):
    tp = fp = fn = incomplete = 0
    for case, report in zip(cases, reports, strict=True):
        if report.get('status') not in ('findings', 'no_findings'):
            incomplete += 1
            continue
        expected = case['expected']
        found = report['findings']
        matched = set()
        for finding in found:
            hit = next((i for i, item in enumerate(expected)
                        if item['criterion'] == finding['criterion']
                        and finding['start'] <= item['line'] <= finding['end']), None)
            if hit is None or hit in matched:
                fp += 1
            else:
                matched.add(hit)
                tp += 1
        fn += len(expected) - len(matched)
    return {'true_positives': tp, 'false_positives': fp, 'false_negatives': fn,
            'incomplete_cases': incomplete, 'cases': len(cases),
            'precision': tp / (tp + fp) if tp + fp else None,
            'recall': tp / (tp + fn) if tp + fn else None,
            'valid_complete_evaluation': incomplete == 0}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True, type=Path)
    parser.add_argument('--live', action='store_true', help='Required acknowledgment of sending fixtures to TypeSafe and using quota')
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if not args.live:
        parser.error('--live is required; this is not an offline accuracy test')
    if not os.environ.get('TYPESAFE_API_KEY'):
        parser.error('TYPESAFE_API_KEY must be supplied explicitly in the environment')
    cases = json.loads((ROOT / 'labels.json').read_text())['cases']
    reports = []
    # No private corpora, credential caches or secret-manager lookup. Fixed destination/model.
    env = {k: v for k, v in os.environ.items() if not k.startswith(('JG_', 'OPENAI_', 'CHATGPT_'))}
    env['JG_NO_FNOX'] = '1'
    for case in cases:
        try:
            run = subprocess.run([str(args.binary.resolve()), '--rules', str(ROOT / 'RULES.md'),
                                  '--source', str(ROOT / case['file']), '--backend', 'jev',
                                  '--model', 'jev-latest', '--base-url', 'https://api.typesafe.ai/v1/systemone',
                                  '--check-budget', '32'], env=env, cwd=ROOT, capture_output=True, text=True, timeout=120)
            try:
                report = json.loads(run.stdout)
            except json.JSONDecodeError:
                report = {'status': 'error', 'error': 'non-JSON output'}
            if run.returncode != 0:
                report['status'] = 'incomplete'
        except (subprocess.TimeoutExpired, OSError):
            report = {'status': 'incomplete', 'error': 'process failed or exceeded 120 second deadline'}
        reports.append(report)
    summary = score(cases, reports)
    args.output.write_text(json.dumps({'provider':'TypeSafe', 'model':'jev-latest',
        'evaluation':'synthetic pilot; not repository coverage or a general accuracy claim',
        'summary':summary, 'cases':cases, 'reports':reports}, indent=2)+'\n')
    print(json.dumps(summary, indent=2))
    return 0 if summary['valid_complete_evaluation'] else 2


if __name__ == '__main__':
    raise SystemExit(main())
