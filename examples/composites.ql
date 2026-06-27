~ Text and other non-numeric values nested inside composites round-trip correctly:
~ a `Text` field of a record, an array of `Text`, and a nested array all read back
~ with their real type (no f64 corruption). Codegen recovers each element/field type
~ from the type oracle rather than assuming `Num`.
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

  nameLen + wordLen + cell        ~ exit 6 + 3 + 3 = 12
>
