~ Built-in `Text` methods (compiler-provided, chainable), each backed by a runtime
~ intrinsic. UTF-8 correct; grapheme-based where an index/length is user-visible.
~   split(sep)                -> []Text   (empty sep -> graphemes; empties preserved)
~   trim()                    -> Text     (strip leading/trailing whitespace)
~   replace(from, to, all)    -> Text     (all=true replaces every match, false the first)
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

  ~ trim strips surrounding whitespace.
  tlen = "  hi  ".trim().size              ~ 2

  ~ replace: all occurrences vs only the first (different lengths make it observable).
  ra = "a-a-a".replace("a", "xx", true).size    ~ "xx-xx-xx" -> 8
  rf = "a-a-a".replace("a", "xx", false).size    ~ "xx-a-a"   -> 6

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

  nparts + first + tlen + ra + rf + chit + cmiss + idx + nidx + sl1 + sl2 + sl3 + up + lo
  ~ 2 + 1 + 2 + 8 + 6 + 1 + 0 + 2 + 3 + 3 + 5 + 0 + 1 + 1 = 35
>
