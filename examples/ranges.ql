~ Ranges: infix `<-` builds an inclusive `[]Num`. It is array sugar — the result
~ IS a `[]Num`, so it has `.size`, indexes with `[i]`, and iterates with `for`.
~   `1 <- 4` -> [1, 2, 3, 4]   (inclusive endpoints)
~   `4 <- 1` -> [4, 3, 2, 1]   (descends when the left end is larger)
<< core.io

^ = () -> Num => <
  asc  = 1 <- 4                 ~ [1, 2, 3, 4]
  desc = 4 <- 1                 ~ [4, 3, 2, 1]

  count = asc.size              ~ 4 (inclusive count = |hi - lo| + 1)

  ~ Ascending: first endpoint is the small end, last is the large end.
  lo = asc[0]                   ~ 1
  hi = asc[3]                   ~ 4

  ~ Descending: the order is reversed — desc[0] is the larger end.
  top    = desc[0]              ~ 4
  bottom = desc[3]              ~ 1

  ~ A range also drives a `for` loop, since it's just a `[]Num`.
  for n <- asc => print(n)      ~ prints 1, 2, 3, 4

  count + lo + hi + top + bottom   ~ 4 + 1 + 4 + 4 + 1 = exit 14
>
