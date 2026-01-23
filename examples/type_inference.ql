~ Type inference examples

~ Simple function with inferred return type
double = (x :: Num) => x * 2

~ Function with inferred parameter types (defaults to Num)
add = (a, b) => a + b

~ Function with full type annotations
greet = (name :: String) -> String => "Hello"

~ Type inference in variables
result = double(21)  ~ inferred as Num
message = greet("Alice")  ~ inferred as String

~ Array type inference
numbers = [1, 2, 3, 4, 5]  ~ inferred as Array(Num)
strings = ["a", "b", "c"]  ~ inferred as Array(String)

~ Record type inference
user = {
    name = "Bob",
    age = 30
}

~ Pipeline with type inference
compute = (x :: Num) => x |> double |> double

~ Block with local variables
factorial = (n :: Num) -> Num => <
    result = 1
    ~ TODO: add loop support
    result
>
