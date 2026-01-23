factorial = (n :: Num) -> Num => n <= 1 ? 1 : n * factorial(n - 1)

>> = () -> Num => factorial(5)
