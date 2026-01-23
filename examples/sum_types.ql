~ Sum types (algebraic data types) example

~ Option type pattern matching
unwrap_or = (opt, default) => opt ?
    | Some(val) => val
    | None => default

~ Using the function
x = unwrap_or(5, 0)  ~ Will need proper Option constructor
y = unwrap_or(10, 42)

~ Result type pattern matching  
handle_result = (res) => res ?
    | Ok(value) => value
    | Err(msg) => 0

~ Pattern matching with wildcards
safe_divide = (a, b) => b ?
    | 0 => None
    | _ => Some(a)

~ Nested pattern matching
process = (opt) => opt ?
    | Some(val) => val ?
        | 0 => "zero"
        | _ => "non-zero"
    | None => "nothing"

~ Custom sum type (will need type declarations in future)
~ For now, we can pattern match on constructor names

color_name = (c) => c ?
    | Red => "red"
    | Green => "green"
    | Blue => "blue"
    | _ => "unknown"
