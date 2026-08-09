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

^ = () -> Num => <
  ~ Record with a Text field: read it back, count its graphemes.
  user = { name = "Quilon", n = 7 }
  nameLen = user.name.length      ~ "Quilon" -> 6

  ~ Array of Text: index it, then take the byte length of the element.
  words = ["a", "cde"]
  wordLen = words[1].size         ~ "cde" -> 3

  ~ Nested array (array of arrays): double-index it.
  grid = [[1, 2], [3, 4]]
  cell = grid[1][0]               ~ 3

  ~ A binding NAME is per-function: `p` is a record in one function and an array
  ~ parameter in another, and each resolves against its own type.
  assertEq(recordP(), 6)
  assertEq(arrayP([1, 2, 3]), 3)

  ~ A closure capture keeps its type too: field reads and method calls on a
  ~ captured record resolve inside the closure body.
  c = Counter { v = 4 }
  readV = () => c.get()
  assertEq(readV(), 4)

  nameLen + wordLen + cell        ~ exit 6 + 3 + 3 = 12
>

