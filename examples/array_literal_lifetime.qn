~ Array literals are heap-backed, so they hold in the two places a stack buffer would break:
~
~   1. Escape — a literal returned from a function (here in a record field) outlives that
~      frame, and a later call reusing the stack region does not corrupt it: `p.xs[0]` stays
~      10 rather than reading the later call's locals.
~   2. Tail recursion — building and indexing a literal each iteration of a self-tail-
~      recursive loop stays in constant stack, so the loop runs 1,000,000 deep instead of
~      overflowing.
~
~ `<< core.test` verifies the results; on success the program exits 0.
<< core.test

Pair = { xs :: []Num }

~ Returns a record holding a fresh array literal — the buffer must outlive this frame.
makePair = () -> Pair => Pair { xs = [10, 20, 30] }

~ Reuses the stack with its own array literal; used to try to clobber `makePair`'s.
clobber = (x :: Num) -> Num => <
  c = [x, x, x]
  c[0]
>

~ Self-tail-recursive: builds and indexes `[1, 2, 3]` each step, summing a[0] (==1)
~ `n` times. Runs a million deep in constant stack because nothing allocas per loop.
sumOnes = (n :: Num, acc :: Num) -> Num => <
  a = [1, 2, 3]
  n <= 0 ? acc : sumOnes(n - 1, acc + a[0])
>

^ = () -> $ => <
  p = makePair()
  z = clobber(77)          ~ overwrite the dead frame with 77s
  assertEq(p.xs[0], 10)    ~ escaped literal is intact, not 77
  assertEq(p.xs.size, 3)

  assertEq(sumOnes(1000000, 0), 1000000)   ~ constant-stack tail loop, no overflow
>
