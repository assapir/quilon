~ `Text` is a built-in type (no import needed, like Num or arrays).
~   "a" + "b"   concatenates (GC-allocated)
~   .size       byte length (UTF-8 bytes)
~   .length     grapheme-cluster count (user-perceived characters)
~ For "héllo" + " 🌍": .size = 11 bytes, .length = 7 graphemes.
~ `<< core.test` verifies both counts; on success the program exits 0.
<< core.test

^ = () -> $ => <
  s :: Text = "héllo" + " 🌍"
  assertEq(s.length, 7)   ~ 7 grapheme clusters
  assertEq(s.size, 11)    ~ 11 UTF-8 bytes
>
