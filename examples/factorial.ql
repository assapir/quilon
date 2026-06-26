~ Recursion with a ternary base case. `->Num` annotation is required for recursion.
factorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)

^ = () -> Num => factorial(5)   ~ 5! = 120
