~ Test the unified Result type with Ok/NotOk
~ Simulates a computation that might succeed or fail
>> = () -> Num => <
  ~ Now we can create Result values
  
  value = 42
  
  ~ Creating and matching on result values
  success = Ok(value)
  failure = NotOk
  
  result1 = success ?
    | Ok(x) => x * 2
    | NotOk => 0
    | _ => -1
  
  result2 = failure ?
    | Ok(x) => x * 2
    | NotOk => 0
    | _ => -1
  
  result1
>
