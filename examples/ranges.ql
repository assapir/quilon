~ Ranges: infix `<-` builds an inclusive `[]Num`. It is array sugar — the result
~ IS a `[]Num`, so it has `.size`, indexes with `[i]`, and iterates with `.each`.
~   `1 <- 4` -> [1, 2, 3, 4]   (inclusive endpoints)
~   `4 <- 1` -> [4, 3, 2, 1]   (descends when the left end is larger)
~ `<< core.test` verifies every result; on success the program exits 0.
<< core.io
<< core.test

^ = () -> $ => <
  asc  :: []Num = 1 <- 4                ~ [1, 2, 3, 4]
  desc :: []Num = 4 <- 1                ~ [4, 3, 2, 1]

  ~ Inclusive count = |hi - lo| + 1.
  assertEq(asc.size, 4)

  ~ Ascending: first endpoint is the small end, last is the large end.
  assertEq(asc[0], 1)
  assertEq(asc[3], 4)

  ~ Descending: the order is reversed — desc[0] is the larger end.
  assertEq(desc[0], 4)
  assertEq(desc[3], 1)

  ~ A range iterates with `.each`, since it's just a `[]Num`.
  asc.each(n => print(n))               ~ prints 1, 2, 3, 4
  assertEq(asc[1], 2)
>
