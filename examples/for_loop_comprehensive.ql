~ Comprehensive for loop examples

~ Example 1: Simple iteration
iterateNumbers = () -> Num => <
  numbers = [1, 2, 3, 4, 5]
  
  numbers |> for n => n
  
  0
>

~ Example 2: Loop with index
iterateWithIndex = () -> Num => <
  items = [10, 20, 30, 40]
  
  items |> for (val, i) => val + i
  
  0
>

~ Example 3: Nested loops
nestedLoops = () -> Num => <
  rows = [1, 2, 3]
  
  ~ Nested iteration with blocks
  rows |> for row => <
    cols = [10, 20, 30]
    cols |> for col => <
      product = row * col
      product
    >
  >
  
  0
>

~ Example 4: Using loop result (returns 0)
loopResult = () -> Num => <
  nums = [5, 10, 15]
  result = nums |> for n => n
  
  ~ result should be 0 (Num)
  check = result + 100
  
  check
>

~ Example 5: Array of arrays
arrayOfArrays = () -> Num => <
  matrix = [
    [1, 2, 3],
    [4, 5, 6],
    [7, 8, 9]
  ]
  
  ~ Nested loops with blocks
  matrix |> for (row, i) => <
    row |> for (val, j) => <
      sum = i + j + val
      sum
    >
  >
  
  0
>

>> = () -> Num => <
  iterateNumbers()
  iterateWithIndex()
  nestedLoops()
  loopResult()
  arrayOfArrays()
  
  0
>
