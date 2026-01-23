~ Test the unified Result type with OK/NotOK
~ Simulates a computation that might succeed or fail
>> = () -> Num => <
  ~ For now we can't actually create Result values yet
  ~ But we can pattern match on them
  
  value = 42
  
  ~ This will be useful once we can create OK(value) and NotOK
  result = value ?
    | OK(x) => x * 2
    | NotOK => 0
    | _ => -1
  
  result
>
