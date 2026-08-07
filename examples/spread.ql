~ Spread `<-` (prefix): splice a source's elements/fields into a literal.
~   Array : `[<-xs, 4, 5]` -> every element of `xs`, then 4, 5 (multiple spreads OK).
~   Record: `{<-p, x = 9}` -> a copy of `p` with `x` overridden (a functional update).
~ Prefix `<-` (first token of an element/field) is SPREAD; infix `lo <- hi` is a range.
~ So `[1 <- 4]` is a ONE-element array holding the range [1,2,3,4], while `[<-xs, 4]`
~ splices xs. Inside a spread the source is a full expression.
<< core.io

~ A named record type — its type identity and methods survive a functional update.
Vec = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y
}

^ = () -> Num => <
  ~ --- Array spread ([]Num) ---
  xs = [1, 2, 3]
  ys = [<-xs, 4, 5]             ~ [1, 2, 3, 4, 5]
  zs = [0, <-xs, <-ys]          ~ [0, 1,2,3, 1,2,3,4,5]  (two spreads, left-to-right)
  nums = ys.size + zs.size      ~ 5 + 9 = 14

  ~ --- Array spread ([]Text) — repr-correct, not just Num ---
  hello = ["h", "e"]
  word  = [<-hello, "l", "l", "o"]   ~ ["h","e","l","l","o"]
  word.each(c => print(c))           ~ prints h e l l o, one per line
  letters = word.size                ~ 5

  ~ --- range vs spread disambiguation ---
  ranges = [1 <- 4]             ~ ONE element: the range [1,2,3,4]
  disamb = ranges.size + ranges[0].size   ~ 1 + 4 = 5

  ~ --- Record functional update (named type + methods preserved) ---
  a = Vec { x = 10, y = 20 }
  b = { <-a, x = 5 }            ~ keeps Vec: b.sum() available
  vecs = b.sum()               ~ 5 + 20 = 25

  ~ --- Record functional update (anonymous, override + add a field) ---
  p = { name = "Ada", tag = 1 }
  q = { <-p, tag = 3, extra = 4 }   ~ override `tag`, add `extra`
  recs = q.tag + q.extra       ~ 3 + 4 = 7

  nums + letters + disamb + vecs + recs   ~ 14 + 5 + 5 + 25 + 7 = exit 56
>
