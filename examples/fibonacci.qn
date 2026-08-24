~ Double recursion. `<< core.test` verifies the result; on success the program exits 0.
<< core.test

fib = (n :: Num) -> Num => n <= 1 ? n : fib(n - 1) + fib(n - 2)

^ = () -> $ => <
  assertEq(fib(10), 55)
  assertEq(fib(1), 1)
>
