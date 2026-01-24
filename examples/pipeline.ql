~ Pipeline example
~ (map, filter, fold will be added as array methods later)

~ Simple pipeline example
double = x :: Num => x * 2
addFive = x :: Num => x + 5

>> = () -> Num => <
  ~ Pipeline operator chains operations
  result = 10 |> double |> addFive
  
  result  ~ Returns 25
>
