~ Test pattern matching with simple patterns
~ Entry point that tests number matching
>> = () -> Num => <
  value = 5
  
  result = value ?
    | 5 => 50
    | 3 => 30
    | _ => 0
  
  result
>
