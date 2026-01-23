~ Pipeline example showing auto-parallelization

numbers = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]

~ This automatically parallelizes!
result = numbers
  |> filter (x => x % 2 == 0)  ~ keep even numbers
  |> map (x => x * x)          ~ square them
  |> fold 0 (acc, x => acc + x) ~ sum them

main = => print "Sum of squares of evens: <result>"
