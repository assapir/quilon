~ Sum types (algebraic data types) example
~ Demonstrates the built-in Result type with Ok/NotOk

~ Creating Result values
success = Ok(42)
failure = NotOk("error message")

~ Pattern matching on numbers
classify = (n) => n ?
    | 0 => "zero"
    | 1 => "one"  
    | 2 => "two"
    | _ => "other"

~ Pattern matching with wildcards
check_positive = (n) => n ?
    | 0 => "zero"
    | _ => "positive or negative"

~ Pattern matching on Results to extract values
~ Note: Inline matching works great!
value1 = (Ok(100)) ?
    | Ok(val) => val
    | NotOk(err) => 0

value2 = (NotOk(999)) ?
    | Ok(val) => val
    | NotOk(err) => -1
