square = (x :: Num) -> Num => x * x

sum_of_squares = (a :: Num, b :: Num) -> Num => square(a) + square(b)

>> = () -> Num => sum_of_squares(3, 4)
