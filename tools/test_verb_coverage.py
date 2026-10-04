"""Regression checks for live setters versus creation/event-only property reads."""
from pathlib import Path
from contextlib import redirect_stdout
from io import StringIO
import tempfile
import unittest
from unittest.mock import patch
import verb_coverage as coverage


class ReapplyTests(unittest.TestCase):
    def scan(self, files):
        with tempfile.TemporaryDirectory() as directory:
            for name, source in files.items():
                Path(directory, name + '.cplus').write_text(source, encoding='utf-8')
            return coverage.reapplied_fields(directory, [('views', 'apply')])

    def test_only_live_call_paths_count(self):
        actual = self.scan({'views': '''
import "./style" as styled;
fn apply(ctx: *u8) { styled::configure(ctx); }
fn create(p: *props::LabelProps) { native.set_wrap((*p).line_break); }
fn changed(p: *props::LabelProps) { native.set_size((*p).font_size); }
''', 'style': '''
fn configure(p: *props::LabelProps) { native.set_text((*p).text.view()); }
fn unused(p: *props::LabelProps) { native.set_color((*p).text_color); }
'''})
        self.assertEqual(actual, {('LabelProps', 'text')})

    def test_scopes_do_not_mix_control_properties(self):
        actual = self.scan({'views': '''
fn apply(raw: *u8) {
  if one { let p: *props::LabelProps=raw as *props::LabelProps;
    native.set_text((*p).text); }
  if two { let p: *props::ButtonProps=raw as *props::ButtonProps;
    native.set_title((*p).title); }
}
'''})
        self.assertEqual(actual, {('LabelProps', 'text'), ('ButtonProps', 'title')})

    def test_reads_without_setters_are_not_implementation(self):
        actual = self.scan({'views': '''
fn apply(p: *props::TextFieldProps) {
  let ignored=(*p).is_secure;
  // native.set_secure((*p).is_secure);
  log("native.set_secure((*p).is_secure)");
  native.set_text((*p).input_view.text);
}
'''})
        self.assertEqual(actual, {('TextFieldProps', 'input_view.text')})

    def test_inferred_pointer_casts_credit_only_reachable_setters(self):
        actual = self.scan({'views': '''
fn apply(raw: *u8) {
  let p=raw as *props::LabelProps;
  native.set_wrap((*p).line_break);
  let ignored=(*p).vertical_align;
}
fn create(raw: *u8) {
  let p=raw as *props::LabelProps;
  native.set_height((*p).line_height);
}
'''})
        self.assertEqual(actual, {('LabelProps', 'line_break')})

    def test_unknown_local_shadows_an_outer_property_pointer(self):
        actual = self.scan({'views': '''
fn apply(p: *props::LabelProps) {
  if condition {let p=unknown; native.set_color((*p).text_color);}
  if condition {let p=Other {x: 0}; native.set_wrap((*p).line_break);}
  if condition {let p=if other {one} else {two}; native.set_size((*p).font_size);}
  native.set_text((*p).text);
}
'''})
        self.assertEqual(actual, {('LabelProps', 'text')})

    def test_cycles_and_shadowed_locals_terminate_without_false_credit(self):
        actual = self.scan({'views': '''
fn apply(p: *props::LabelProps) {
  recurse();
  if true {let p: *props::ButtonProps=other; native.set_title((*p).title);}
  native.set_text((*p).text);
}
fn recurse() { apply(other); }
'''})
        self.assertEqual(actual, {('LabelProps', 'text'), ('ButtonProps', 'title')})


class ReviewedLedgerTests(unittest.TestCase):
    def report(self, manifest, measured):
        with tempfile.TemporaryDirectory() as directory:
            package = Path(directory, 'vendor/facet_winui')
            package.mkdir(parents=True)
            (package / 'MANIFEST.md').write_text(manifest, encoding='utf-8')
            output = StringIO()
            with patch.object(coverage, 'ROOT', directory), \
                    patch.object(coverage, 'buckets', return_value=measured), \
                    patch.object(coverage, 'handler_buckets', return_value=([], [], [])), \
                    redirect_stdout(output):
                result = coverage.report('winui', show_list=True)
            return result, output.getvalue()

    def test_reviewed_reads_are_separate_from_automatic_live_and_debt(self):
        result, output = self.report(
            '```reviewed-live\nlabel.line_height helper source audit\n```\n',
            (['label.text'], ['label.line_height', 'label.line_break'], [], [], []))
        self.assertEqual(result['total'], 3)
        self.assertEqual(result['live'], 1)
        self.assertEqual(result['reviewed_live'], 1)
        self.assertEqual(result['absent'], ['label.line_break'])
        self.assertEqual(result['contradicted'], [])
        self.assertIn('REVIEWED-LIVE (1)', output)
        # A backend-specific source audit is not a portable design disposition.
        self.assertNotIn('label.line_height', result['recorded'])

    def test_reviewed_entry_without_a_read_remains_debt(self):
        result, _ = self.report(
            '```reviewed-live\nlabel.line_height obsolete evidence\n```\n',
            ([], [], ['label.line_height'], [], []))
        self.assertEqual(result['reviewed_live'], 0)
        self.assertEqual(result['absent'], ['label.line_height'])
        self.assertEqual(result['contradicted'], ['label.line_height'])
        self.assertEqual(result['stale'], [])

    def test_nonexistent_reviewed_entry_is_stale(self):
        result, _ = self.report(
            '```reviewed-live\nlabel.typo evidence\n```\n',
            ([], [], [], [], []))
        self.assertEqual(result['stale'], ['label.typo'])
        self.assertEqual(result['reviewed_live'], 0)

    def test_new_automatic_detection_does_not_double_count_review(self):
        result, _ = self.report(
            '```reviewed-live\nlabel.line_height source audit\n```\n',
            (['label.line_height'], [], [], [], []))
        self.assertEqual(result['total'], 1)
        self.assertEqual(result['live'], 1)
        self.assertEqual(result['reviewed_live'], 0)
        self.assertEqual(result['stale'], [])
        self.assertEqual(result['contradicted'], [])

    def test_review_cannot_hide_a_gated_unread_field(self):
        result, _ = self.report(
            '```reviewed-live\nlabel.line_height unsupported claim\n```\n',
            ([], [], [], [], ['label.line_height']))
        self.assertEqual(result['reviewed_live'], 0)
        self.assertEqual(result['unread'], ['label.line_height'])
        self.assertEqual(result['contradicted'], ['label.line_height'])
        self.assertEqual(result['stale'], [])


if __name__ == '__main__':
    unittest.main()
