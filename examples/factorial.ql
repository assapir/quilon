~ Factorial example showing recursion and pattern matching

factorial = n :: Num => n ?
  | 0 => 1
  | n => n * factorial (n - 1)

main = => <
  result = factorial 5
  print "Factorial of 5 is: <result>"
>
