~ Test pattern matching fallthrough to wildcard
>> = () -> Num => <
  value = 7
  
  result = value ?
    | 5 => 50
    | 3 => 30
    | _ => 99
  
  result
>
