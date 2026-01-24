~ Sum types (algebraic data types) example
~ Demonstrates the built-in Result type with Ok/NotOk

~ Creating Result values
success = Ok(42)
failure = NotOk("error message")

~ Pattern matching on numbers (simple case)
classify = (n) => n ?
    | 0 => "zero"
    | 1 => "one"  
    | 2 => "two"
    | _ => "other"

~ Pattern matching with wildcards
check_positive = (n) => n ?
    | 0 => "zero"
    | _ => "positive or negative"

~ Returning Result from pattern match
~ Note: Type inference limitations mean we can't easily
~ pass Result values to untyped function parameters
~ This will be improved in future versions

~ For now, demonstrate construction
make_success = () => Ok(100)
make_failure = () => NotOk(404)
