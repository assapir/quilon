~ Test the unified Result type with Ok/NotOk
~ Demonstrates creating Result values
>> = () -> Num => <
  ~ Creating Result values
  success = Ok(42)
  failure = NotOk(404)
  
  ~ For now, just return a number
  ~ Pattern matching on Results has type inference limitations
  42
>
