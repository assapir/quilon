~ Spread `<-` (prefix): splice a source's elements/fields into a literal.
~   Array : `[<-xs, 4, 5]` -> every element of `xs`, then 4, 5 (multiple spreads OK).
~   Record: `{<-p, x = 9}` -> a copy of `p` with `x` overridden (a functional update).
~ Prefix `<-` (first token of an element/field) is SPREAD; infix `lo <- hi` is a range.
~ So `[1 <- 4]` is a ONE-element array holding the range [1,2,3,4], while `[<-xs, 4]`
~ splices xs. Inside a spread the source is a full expression.
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.io
<< core.test

~ A named record type — its type identity and methods survive a functional update.
Vec = {
  x :: Num,
  y :: Num,
  sum = => it.x + it.y
}

^ = () -> $ => <
  ~ --- Array spread ([]Num) ---
  xs :: []Num = [1, 2, 3]
  ys :: []Num = [<-xs, 4, 5]           ~ [1, 2, 3, 4, 5]
  zs :: []Num = [0, <-xs, <-ys]        ~ [0, 1,2,3, 1,2,3,4,5]  (two spreads, left-to-right)
  assertEq(ys.size, 5)
  assertEq(zs.size, 9)

  ~ --- Array spread ([]Text) — repr-correct, not just Num ---
  hello :: []Text = ["h", "e"]
  word  :: []Text = [<-hello, "l", "l", "o"]   ~ ["h","e","l","l","o"]
  word.each(c => print(c))                     ~ prints h e l l o, one per line
  assertEq(word.size, 5)
  assertEq(word[4], "o")

  ~ --- range vs spread disambiguation ---
  ranges :: [][]Num = [1 <- 4]         ~ ONE element: the range [1,2,3,4]
  assertEq(ranges.size, 1)
  assertEq(ranges[0].size, 4)

  ~ --- Record functional update (named type + methods preserved) ---
  a :: Vec = Vec { x = 10, y = 20 }
  b :: Vec = { <-a, x = 5 }            ~ keeps Vec: b.sum() available
  assertEq(b.sum(), 25)                ~ 5 + 20

  ~ --- Record functional update (anonymous, override + add a field) ---
  p = { name = "Ada", tag = 1 }
  q = { <-p, tag = 3, extra = 4 }      ~ override `tag`, add `extra`
  assertEq(q.tag, 3)
  assertEq(q.extra, 4)
>
