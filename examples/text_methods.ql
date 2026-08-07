~ Built-in `Text` methods (compiler-provided, chainable), each backed by a runtime
~ intrinsic. UTF-8 correct; grapheme-based where an index/length is user-visible.
~   split(sep)                        -> []Text  (empty sep -> graphemes; empties preserved)
~   trim() / trimStart() / trimEnd()  -> Text    (strip both / leading / trailing whitespace)
~   replaceAll(from, to)              -> Text    (replace every occurrence)
~   replace(from, to, count)          -> Text    (replace exactly the first `count`; count > 0)
~   contains(sub)                     -> Bool
~   indexOf(sub)                      -> Ok(Num) grapheme index / NotOk   (no -1 sentinel)
~   slice(start, end)                 -> Text    (grapheme indices, clamped; end exclusive)
~   toUpper() / toLower()             -> Text    (Unicode-aware case mapping)
^ = () -> Num => <
  s :: Text = "Hello, World"

  ~ "Hello, World" splits on ", " into ["Hello", "World"]; at(0) reads the first piece.
  parts :: []Text = s.split(", ")
  nparts :: Num = parts.size                ~ 2
  first :: Num = parts.at(0) ?
    | Ok(w)    => w == "Hello" ? 1 : 0       ~ the piece is a Text, equal to "Hello"
    | NotOk(_) => 0

  ~ trim strips both sides; trimStart / trimEnd strip one side only.
  trimmed :: Text = "  hi  ".trim()          ~ "hi"
  tlen :: Num = trimmed.size                 ~ 2
  ts :: Num = "  hi  ".trimStart().size      ~ "hi  " -> 4
  te :: Num = "  hi  ".trimEnd().size        ~ "  hi" -> 4

  ~ replaceAll rewrites every match; replace(count) rewrites exactly the first `count`
  ~ (a literal count <= 0, an over-count, or an empty `from` is a compile error).
  ra :: Num = "a-a-a".replaceAll("a", "xx").size       ~ "xx-xx-xx" -> 8
  rf :: Num = "a-a-a".replace("a", "xx", 1).size       ~ "xx-a-a"   -> 6

  ~ contains: "Hello, World" contains "World" but not "zzz".
  hasWorld :: Bool = s.contains("World")     ~ true
  chit :: Num = hasWorld ? 1 : 0             ~ 1
  cmiss :: Num = s.contains("zzz") ? 10 : 0  ~ 0

  ~ indexOf: Ok(grapheme index) when found, NotOk when absent.
  idx :: Num = "Hello".indexOf("llo") ?      ~ Ok(2)
    | Ok(i)    => i                           ~ 2
    | NotOk(_) => 0
  nidx :: Num = "Hello".indexOf("z") ?       ~ NotOk
    | Ok(_)    => 50
    | NotOk(_) => 3                           ~ 3

  ~ slice over grapheme indices, end exclusive; out-of-range indices clamp.
  sl1 :: Num = "Hello".slice(1, 4).size      ~ "ell"  -> 3
  sl2 :: Num = "Hello".slice(-5, 100).size   ~ clamps to the whole string -> 5
  sl3 :: Num = "Hello".slice(3, 1).size      ~ empty (end <= start) -> 0

  ~ toUpper / toLower map case; compared here by content equality.
  up :: Num = "abc".toUpper() == "ABC" ? 1 : 0   ~ 1
  lo :: Num = "ABC".toLower() == "abc" ? 1 : 0   ~ 1

  ~ Multibyte content, with grapheme-based indices throughout:
  usplit :: Num = "a🌍b🌍c".split("🌍").size      ~ splits on the 4-byte emoji -> ["a","b","c"], 3
  uidx :: Num = "a🌍b".indexOf("b") ?          ~ "b" is grapheme 2 (past the 4-byte 🌍)
    | Ok(i)    => i                             ~ 2
    | NotOk(_) => 99
  uslice :: Num = "héllo".slice(1, 3) == "él" ? 1 : 0   ~ "él" — no codepoint is split mid-byte
  ucont :: Num = "a🌍b".contains("🌍") ? 1 : 0          ~ matches the multibyte substring -> 1
  usharp :: Num = "ß".toUpper() == "SS" ? 1 : 0         ~ "ß" uppercases to two characters "SS"

  ~ []Text is a plain generic array: map/reduce over its Text elements ...
  gmap :: Num = "aa,b,ccc".split(",").map(w => w.size).reduce(0, (a, x) => a + x)   ~ 2+1+3 = 6
  ~ ... and `+` concatenates two []Text into one.
  cat :: []Text = "a,b".split(",") + "c,d".split(",")   ~ ["a", "b", "c", "d"]
  gcat :: Num = cat.size                     ~ 4

  nparts + first + tlen + ts + te + ra + rf + chit + cmiss + idx + nidx + sl1 + sl2 + sl3
    + up + lo + usplit + uidx + uslice + ucont + usharp + gmap + gcat
  ~ = 61
>
