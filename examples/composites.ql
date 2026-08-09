~ Text and other non-numeric values nested inside composites round-trip correctly:
~ a `Text` field of a record, an array of `Text`, and a nested array all read back
~ with their real type (no f64 corruption). Codegen recovers each element/field type
~ from the type oracle rather than assuming `Num`.
<< core.test

Counter = { v :: Num, get = () -> Num => it.v }

~ Binds a RECORD to `p`...
recordP = () -> Num => <
  p = { size = 5, other = 6 }
  p.other
>

~ ...while here `p` is an ARRAY parameter; `.size` is the array's length.
arrayP = (p :: []Num) -> Num => p.size

^ = () -> $ => <
  ~ Record with a Text field: read it back, count its graphemes.
  user = { name = "Quilon", n = 7 }
  assertEq(user.name.length, 6)   ~ "Quilon" -> 6

  ~ Array of Text: index it, then take the byte length of the element.
  words :: []Text = ["a", "cde"]
  assertEq(words[1].size, 3)      ~ "cde" -> 3

  ~ Nested array (array of arrays): double-index it.
  grid :: [][]Num = [[1, 2], [3, 4]]
  assertEq(grid[1][0], 3)

  ~ A binding NAME is per-function: `p` is a record in one function and an array
  ~ parameter in another, and each resolves against its own type.
  assertEq(recordP(), 6)
  assertEq(arrayP([1, 2, 3]), 3)

  ~ A closure capture keeps its type too: field reads and method calls on a
  ~ captured record resolve inside the closure body.
  c :: Counter = Counter { v = 4 }
  readV = () => c.get()
  assertEq(readV(), 4)
>

