~ `Text` is a built-in type (no import needed, like Num or arrays).
~   "a" + "b"   concatenates (GC-allocated)
~   .size       byte length (UTF-8 bytes)
~   .length     grapheme-cluster count (user-perceived characters)
~ For "héllo" + " 🌍": .size = 11 bytes, .length = 7 graphemes.
^ = () -> Num => ("héllo" + " 🌍").length   ~ exit 7
