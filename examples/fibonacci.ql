fib = (n :: Num) -> Num => n <= 1 ? n : fib(n - 1) + fib(n - 2)

quilon_main = () -> Num => fib(10)
