~ Demonstrate Result pattern matching
~ Shows Ok/NotOk constructors and pattern extraction

>> = () -> Num => <
    ~ Create Result values
    success = Ok(42)
    failure = NotOk(404)
    
    ~ Pattern match to extract values
    value1 = success ?
        | Ok(x) => x * 2      ~ Extract successful value
        | NotOk(e) => 0       ~ Handle error case
    
    value2 = failure ?
        | Ok(x) => x
        | NotOk(e) => e + 1   ~ Extract error value
    
    ~ Inline Result creation and matching
    value3 = (Ok(10)) ?
        | Ok(x) => x + 5
        | NotOk(e) => 0
    
    ~ Pattern matching on numbers still works
    check = 5 ?
        | 0 => 999
        | 5 => 123
        | _ => -1
    
    ~ Return sum: 84 + 405 + 15 + 123 = 627
    value1 + value2 + value3 + check
>
