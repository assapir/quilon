~ Guaranteed self-tail-call optimization: a self-call in tail position is lowered to
~ a loop, so this recurses 1,000,000 deep in constant stack. Without the optimization
~ the same program would overflow the stack and crash.
~
~ `count` tail-recurses with an accumulator (the recursive call is the whole value of
~ the `:` branch — i.e. it is in tail position), counting `n` down to 0. `acc` cycles
~ through 0..250 so the result stays a small, deterministic exit code without needing
~ `%` on the result: it equals (number of steps) mod 251 = 1_000_000 mod 251 = 16.
count = (n :: Num, acc :: Num) -> Num =>
  n == 0 ? acc : count(n - 1, acc == 250 ? 0 : acc + 1)

^ = () -> Num => count(1000000, 0)   ~ 1_000_000 mod 251 = 16
