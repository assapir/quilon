~ `+` on arrays builds a NEW array (it NEVER mutates an operand), in three forms —
~ each picked by the EXACT operand types, so there is never any ambiguity:
~   concat:  []T + []T -> []T   ([1,2] + [3,4] -> [1,2,3,4])
~   append:  []T + T   -> []T   ([1,2] + 5     -> [1,2,5])
~   prepend: T + []T   -> []T   (0 + [1,2]     -> [0,1,2])
~ Works for every element type ([]Num, []Text, nested arrays), and for nested arrays
~ `[][]Num + []Num` is an APPEND (the []Num is a single element), not a concat.
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.io
<< core.test

^ = () -> $ => <
  ~ --- concat ([]Num) ---
  a :: []Num = [1, 2]
  b :: []Num = [3, 4]
  c :: []Num = a + b                   ~ [1, 2, 3, 4]
  assertEq(c.size, 4)

  ~ --- append / prepend ([]Num); the operands are left untouched ---
  appended :: []Num = a + 5            ~ [1, 2, 5]
  prepended :: []Num = 0 + a           ~ [0, 1, 2]
  chain :: []Num = 0 + a + 9           ~ ((0 + a) + 9) = [0, 1, 2, 9]
  assertEq(appended.size, 3)
  assertEq(prepended.size, 3)
  assertEq(chain.size, 4)
  assertEq(a.size, 2)                  ~ still 2 — `a` was not mutated by any `+` above

  ~ --- concat / append / prepend ([]Text) — repr-correct, not just Num ---
  hi :: []Text = ["h", "e"] + ["l", "l", "o"]   ~ ["h", "e", "l", "l", "o"]
  hi.each(w => print(w))               ~ prints h e l l o, one per line
  named :: []Text = ["Ada"] + "Lovelace"        ~ append -> ["Ada", "Lovelace"]
  greet :: []Text = "Hi" + ["there"]            ~ prepend -> ["Hi", "there"]
  assertEq(hi.size, 5)
  assertEq(named.size, 2)
  assertEq(named[1], "Lovelace")
  assertEq(greet.size, 2)
  assertEq(greet[0], "Hi")

  ~ --- nested arrays: [][]Num + []Num is APPEND (one row), + [][]Num is CONCAT ---
  rows :: [][]Num = [[1, 2], [3, 4]]
  rows2 :: [][]Num = rows + [5, 6]     ~ append a row -> [[1,2], [3,4], [5,6]]
  grid :: [][]Num = rows + rows        ~ concat -> [[1,2], [3,4], [1,2], [3,4]]
  assertEq(rows2.size, 3)
  assertEq(grid.size, 4)
  assertEq(rows2[2][1], 6)
>
