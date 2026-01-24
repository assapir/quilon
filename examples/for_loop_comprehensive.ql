~ Comprehensive for loop examples

~ Example 1: Simple iteration
printNumbers = () -> Num => <
  numbers = [1, 2, 3, 4, 5]
  
  numbers |> for n => <
    print(n)
  >
  
  0
>

~ Example 2: Loop with index
printWithIndex = () -> Num => <
  items = [10, 20, 30, 40]
  
  items |> for (val, i) => <
    print(i)
    print(val)
  >
  
  0
>

~ Example 3: Nested loops
nestedLoops = () -> Num => <
  rows = [1, 2, 3]
  
  rows |> for row => <
    cols = [10, 20, 30]
    cols |> for col => <
      product = row * col
      print(product)
    >
  >
  
  0
>

~ Example 4: Using loop result (returns 0)
loopResult = () -> Num => <
  nums = [5, 10, 15]
  result = nums |> for n => print(n)
  
  ~ result should be 0 (Num)
  check = result + 100
  print(check)
  
  0
>

~ Example 5: Array of arrays
arrayOfArrays = () -> Num => <
  matrix = [
    [1, 2, 3],
    [4, 5, 6],
    [7, 8, 9]
  ]
  
  matrix |> for (row, i) => <
    row |> for (val, j) => <
      print(val)
    >
  >
  
  0
>

>> = () -> Num => <
  printNumbers()
  printWithIndex()
  nestedLoops()
  loopResult()
  arrayOfArrays()
  
  0
>
