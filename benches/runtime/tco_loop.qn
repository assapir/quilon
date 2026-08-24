~ A tail-recursive countdown — lowered to a loop, so this times raw iteration.
count = (n :: Num, acc :: Num) -> Num => n == 0 ? acc : count(n - 1, acc + 1)
^ = () -> Num => count(50000000, 0) > 0 ? 0 : 1
