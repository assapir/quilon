~ What a fail-loud RUNTIME failure tells you — the second example here that does NOT exit 0.
~
~ Quilon refuses to read past the end of an array: no clamping, no zero, no raw memory. The
~ read fails loudly, and says which read it was — the same framed report a failing assertion
~ (see `examples/assert_location.ql`) or a compile error gives you.
~
~ Run: cargo run -- run examples/index_out_of_bounds.ql   ~ exits 1, and prints to stderr:
~
~   examples/index_out_of_bounds.ql:21:11: index 7 out of bounds for an array of size 3
~      |
~   21 |   value = items()[wanted]
~      |           ^^^^^^^^^^^^^^^
~
~ Use `.at(i)` instead when an index may be out of range: it answers `Ok(elem)` / `NotOk`
~ rather than failing, so the program decides what to do (see `examples/array_methods.ql`).
<< core.test

items = () -> []Num => [1, 2, 3]

readAt = (wanted :: Num) -> Num => <
  value = items()[wanted]
  value
>

^ = () -> $ => <
  ~ In range: fine.
  assertEq(readAt(0), 1)

  ~ Out of range: fails right here, naming the read above.
  assertEq(readAt(7), 0)
>
