"""Unit tests for the lldb formatter's render-thunk symbol derivation.

Run with `python3 -m unittest editors/vscode/formatters/test_quilon.py` (or
`python3 -m unittest discover -s editors/vscode/formatters`) — no `lldb` module needed:
`quilon.py` guards that import so `sanitize_debug_type_name`/`render_thunk_symbol` can be
exercised standalone.

The example table below is the SAME one `src/codegen/debug.rs`'s `symbol_tests` module
checks against `sanitize_debug_type_name`/`render_thunk_symbol` on the Rust side — the two
implementations must derive the identical symbol from the identical DWARF display name, or a
debugger would call the wrong type's render thunk. Keep the two tables in lockstep.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from quilon import render_thunk_symbol, sanitize_debug_type_name  # noqa: E402

EXAMPLES = [
    ("Num", "__qn_render$Num"),
    ("Bool", "__qn_render$Bool"),
    ("Text", "__qn_render$Text"),
    ("$", "__qn_render$$"),
    ("[]Num", "__qn_render$$Num"),
    ("[][]Text", "__qn_render$$$Text"),
    ("Point", "__qn_render$Point"),
    ("Result", "__qn_render$Result"),
    ("Map[Text, Num]", "__qn_render$Map$Text$Num"),
    ("Set[Num]", "__qn_render$Set$Num"),
]


class RenderThunkSymbolTests(unittest.TestCase):
    def test_matches_the_shared_example_table(self):
        for debug_name, expected in EXAMPLES:
            with self.subTest(debug_name=debug_name):
                self.assertEqual(render_thunk_symbol(debug_name), expected)

    def test_a_single_and_a_doubly_nested_array_do_not_collide(self):
        self.assertNotEqual(
            render_thunk_symbol("[]Text"), render_thunk_symbol("[][]Text")
        )

    def test_sanitize_drops_qualified_name_dots(self):
        # A qualified sum/record name (an imported module's type) has no `.` in the
        # sanitized suffix — dots are dropped like any other non-alnum separator.
        self.assertEqual(sanitize_debug_type_name("core.http.Get"), "corehttpGet")


if __name__ == "__main__":
    unittest.main()
