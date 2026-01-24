~ Simple for loop test without blocks in loop body

test1 = () -> Num => <
  numbers = [1, 2, 3]
  numbers |> for n => n
  0
>

test2 = () -> Num => <
  items = [10, 20, 30]
  items |> for (val, i) => val
  0
>

>> = () -> Num => <
  test1()
  test2()
  0
>
