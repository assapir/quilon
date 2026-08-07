~ Built-in `Text` methods (compiler-provided, chainable), each backed by a runtime
~ intrinsic. UTF-8 correct; grapheme-based where an index/length is user-visible.
~   split(sep)                -> []Text   (empty sep -> graphemes; empties preserved)
~   trim() / trimStart() / trimEnd() -> Text   (strip both / leading / trailing whitespace)
~   replace(from, to, { all = Bool }) -> Text  (all=true replaces every match, false the first)
~   contains(sub)             -> Bool
~   indexOf(sub)              -> Ok(Num) grapheme index / NotOk   (no -1 sentinel)
~   slice(start, end)         -> Text     (grapheme indices, clamped; end exclusive)
~   toUpper() / toLower()     -> Text     (Unicode-aware case mapping)
^ = () -> Num => <
  s = "Hello, World"

  ~ split on ", " -> ["Hello", "World"]; then read an element back as a real Text.
  parts = s.split(", ")
  nparts = parts.size                     ~ 2
  first = parts.at(0) ?                    ~ Ok("Hello")
    | Ok(w)    => w == "Hello" ? 1 : 0     ~ 1  (split pieces are genuine Text)
    | NotOk(_) => 0

  ~ trim strips both sides; trimStart / trimEnd strip one side only.
  tlen = "  hi  ".trim().size              ~ 2
  ts = "  hi  ".trimStart().size           ~ "hi  " -> 4
  te = "  hi  ".trimEnd().size             ~ "  hi" -> 4

  ~ replace takes an options record { all :: Bool }: all occurrences vs only the first
  ~ (different from/to lengths make the choice observable).
  ra = "a-a-a".replace("a", "xx", { all = true }).size     ~ "xx-xx-xx" -> 8
  rf = "a-a-a".replace("a", "xx", { all = false }).size    ~ "xx-a-a"   -> 6

  ~ contains: a hit contributes, a miss must contribute nothing.
  chit  = s.contains("World") ? 1 : 0      ~ 1
  cmiss = s.contains("zzz") ? 10 : 0       ~ 0  (a false hit would add 10)

  ~ indexOf: Ok(grapheme index) when found, NotOk when absent.
  idx = "Hello".indexOf("llo") ?           ~ Ok(2)
    | Ok(i)    => i                         ~ 2
    | NotOk(_) => 0
  nidx = "Hello".indexOf("z") ?            ~ NotOk
    | Ok(_)    => 50                        ~ a wrong Ok would add 50
    | NotOk(_) => 3                         ~ 3

  ~ slice: grapheme indices, clamped to bounds, end exclusive.
  sl1 = "Hello".slice(1, 4).size           ~ "ell"  -> 3
  sl2 = "Hello".slice(-5, 100).size        ~ clamp  -> 5
  sl3 = "Hello".slice(3, 1).size           ~ empty  -> 0

  ~ case mapping, verified by content equality.
  up = "abc".toUpper() == "ABC" ? 1 : 0    ~ 1
  lo = "ABC".toLower() == "abc" ? 1 : 0    ~ 1

  ~ --- Unicode correctness: multibyte content, grapheme-based indices ---
  ~ split on a 4-byte emoji separator -> ["a","b","c"].
  usplit = "a🌍b🌍c".split("🌍").size      ~ 3
  ~ indexOf returns a GRAPHEME index: "b" is grapheme 2 (past the 4-byte 🌍), not byte 5.
  uidx = "a🌍b".indexOf("b") ?
    | Ok(i)    => i                         ~ 2
    | NotOk(_) => 99
  ~ slice over graphemes never splits a multibyte codepoint mid-byte.
  uslice = "héllo".slice(1, 3) == "él" ? 1 : 0   ~ 1
  ~ contains matches a multibyte substring.
  ucont = "a🌍b".contains("🌍") ? 1 : 0    ~ 1
  ~ Unicode-aware case mapping, incl. the 1->N mapping "ß" -> "SS".
  usharp = "ß".toUpper() == "SS" ? 1 : 0   ~ 1

  nparts + first + tlen + ts + te + ra + rf + chit + cmiss + idx + nidx + sl1 + sl2 + sl3
    + up + lo + usplit + uidx + uslice + ucont + usharp
  ~ 43 (prior block) + ts 4 + te 4 = 51
>
