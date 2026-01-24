~ Sum types (algebraic data types) example

~ Option type pattern matching
unwrap_or = (opt, default) => opt ?
    | Ok(val) => val
    | NotOk => default

~ Using the function
x = unwrap_or(Ok(5), 0)
y = unwrap_or(NotOk, 42)

~ Result type pattern matching  
handle_result = (res) => res ?
    | Ok(value) => value
    | NotOk => 0

~ Pattern matching with wildcards
safe_divide = (a, b) => b ?
    | 0 => NotOk
    | _ => Ok(a)

~ Nested pattern matching
process = (opt) => opt ?
    | Ok(val) => val ?
        | 0 => "zero"
        | _ => "non-zero"
    | NotOk => "nothing"

~ Custom sum type (will need type declarations in future)
~ For now, we can pattern match on constructor names

color_name = (c) => c ?
    | Red => "red"
    | Green => "green"
    | Blue => "blue"
    | _ => "unknown"
