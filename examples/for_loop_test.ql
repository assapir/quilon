~ Test for loop feature

~ Simple for loop over array
testSimpleLoop = () -> Num => <
  nums = [1, 2, 3, 4, 5]
  
  ~ Loop with just item binding
  nums |> for n => n
  
  0
>

~ For loop with index
testLoopWithIndex = () -> Num => <
  items = [10, 20, 30]
  
  ~ Loop with (item, index) binding
  items |> for (val, i) => val + i
  
  0
>

>> = () -> Num => <
  testSimpleLoop()
  testLoopWithIndex()
  
  0
>
