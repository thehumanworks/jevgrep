"""Metric arithmetic and corpus integrity only; no model accuracy claims."""
import importlib.util
import json
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('rule_evaluation', ROOT / 'bench/rules/evaluate.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class RuleEvaluationTests(unittest.TestCase):
    def test_localization_duplicates_false_positives_and_incomplete(self):
        cases = [{'expected': [{'criterion': 'A', 'line': 4}]}, {'expected': []}, {'expected': []}]
        reports = [
            {'status': 'findings', 'findings': [
                {'criterion': 'A', 'start': 3, 'end': 5},
                {'criterion': 'A', 'start': 3, 'end': 5},
                {'criterion': 'A', 'start': 8, 'end': 9}]},
            {'status': 'findings', 'findings': [{'criterion': 'B', 'start': 1, 'end': 1}]},
            {'status': 'incomplete', 'findings': []}]
        result = MODULE.score(cases, reports)
        self.assertEqual((result['true_positives'], result['false_positives'], result['false_negatives']), (1, 3, 0))
        self.assertEqual(result['precision'], 0.25)
        self.assertFalse(result['valid_complete_evaluation'])
        self.assertEqual(result['incomplete_cases'], 1)

    def test_missed_violation_is_false_negative(self):
        result = MODULE.score([{'expected': [{'criterion': 'A', 'line': 4}]}],
                              [{'status': 'no_findings', 'findings': []}])
        self.assertEqual(result['false_negatives'], 1)
        self.assertEqual(result['recall'], 0)
        self.assertTrue(result['valid_complete_evaluation'])

    def test_corpus_is_labelled_across_languages_and_rules(self):
        cases = json.loads((ROOT / 'bench/rules/labels.json').read_text())['cases']
        self.assertEqual(len(cases), 18)
        self.assertEqual(sum(not case['expected'] for case in cases), 9)
        for ext in ('py', 'ts', 'rs'):
            selected = [case for case in cases if case['file'].endswith('.' + ext)]
            self.assertEqual(len(selected), 6)
            self.assertEqual({label['criterion'] for case in selected for label in case['expected']}, {'NAME', 'RESP', 'FAIL'})
            for case in selected:
                self.assertRegex(case['file'], r'^cases/case-[0-9]+\.(py|ts|rs)$')
                lines = (ROOT / 'bench/rules' / case['file']).read_text().splitlines()
                for label in case['expected']:
                    self.assertTrue(1 <= label['line'] <= len(lines))


if __name__ == '__main__':
    unittest.main()
