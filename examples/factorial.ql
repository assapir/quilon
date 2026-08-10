~ Recursion with a ternary base case. `->Num` annotation is required for recursion.
~ `<< core.test` verifies the result; on success the program exits 0.
<< core.test

factorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)

^ = () -> $ => <
  assertEq(factorial(5), 120)   ~ 5! = 120
  assertEq(factorial(0), 1)     ~ base case
>
