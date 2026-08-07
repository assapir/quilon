~ `+` on arrays builds a NEW array (it NEVER mutates an operand), in three forms —
~ each picked by the EXACT operand types, so there is never any ambiguity:
~   concat:  []T + []T -> []T   ([1,2] + [3,4] -> [1,2,3,4])
~   append:  []T + T   -> []T   ([1,2] + 5     -> [1,2,5])
~   prepend: T + []T   -> []T   (0 + [1,2]     -> [0,1,2])
~ Works for every element type ([]Num, []Text, nested arrays), and for nested arrays
~ `[][]Num + []Num` is an APPEND (the []Num is a single element), not a concat.
<< core.io

^ = () -> Num => <
  ~ --- concat ([]Num) ---
  a = [1, 2]
  b = [3, 4]
  c = a + b                    ~ [1, 2, 3, 4]
  nums = c.size                ~ 4

  ~ --- append / prepend ([]Num); the operands are left untouched ---
  appended = a + 5             ~ [1, 2, 5]
  prepended = 0 + a            ~ [0, 1, 2]
  chain = 0 + a + 9            ~ ((0 + a) + 9) = [0, 1, 2, 9]
  more = appended.size + prepended.size + chain.size   ~ 3 + 3 + 4 = 10
  untouched = a.size           ~ still 2 — `a` was not mutated by any `+` above

  ~ --- concat / append / prepend ([]Text) — repr-correct, not just Num ---
  hi = ["h", "e"] + ["l", "l", "o"]   ~ ["h", "e", "l", "l", "o"]
  hi.each(w => print(w))              ~ prints h e l l o, one per line
  named = ["Ada"] + "Lovelace"        ~ append -> ["Ada", "Lovelace"]
  greet = "Hi" + ["there"]            ~ prepend -> ["Hi", "there"]
  texts = hi.size + named.size + greet.size   ~ 5 + 2 + 2 = 9

  ~ --- nested arrays: [][]Num + []Num is APPEND (one row), + [][]Num is CONCAT ---
  rows = [[1, 2], [3, 4]]
  rows2 = rows + [5, 6]        ~ append a row -> [[1,2], [3,4], [5,6]]
  grid = rows + rows           ~ concat -> [[1,2], [3,4], [1,2], [3,4]]
  nested = rows2.size + grid.size + rows2[2][1]   ~ 3 + 4 + 6 = 13

  nums + more + untouched + texts + nested   ~ 4 + 10 + 2 + 9 + 13 = exit 38
>
