"""Regression checks for live setters versus creation/event-only property reads."""
from pathlib import Path
import tempfile
import unittest
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


if __name__ == '__main__':
    unittest.main()
