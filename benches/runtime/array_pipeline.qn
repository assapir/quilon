~ Range materialization plus map/filter/reduce — three passes, each allocating.
^ = () -> Num => <
  xs = 1 <- 2000000
  total = xs.map(x => x * 2).filter(x => x > 100).reduce(0, (a, x) => a + x)
  total > 0 ? 0 : 1
>
