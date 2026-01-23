# Quilon Language Syntax Specification

## Overview

Quilon uses a minimal, symbolic syntax with no curly braces or significant indentation.

## Comments

```quilon
~ This is a single-line comment
```

## Variables

No keywords needed - just assignment:

```quilon
name = "Alice"
age = 30
active = true
```

With type annotation:

```quilon
port :: Num = 3000
```

Mutable variables use `mut` prefix:

```quilon
mut counter = 0
counter = counter + 1
```

## Functions

Functions are arrow assignments:

```quilon
~ Single expression
greet = name :: String => "Hello, <name>!"

~ With explicit return type
add = (a :: Num, b :: Num) -> Num => a + b

~ Multi-line block with < >
process = data :: []Num => <
  data
    |> filter (x => x > 0)
    |> map (x => x * 2)
    |> collect
>
```

## Types

### Core Types

- `Num` - All numbers (int/float inferred)
- `String` - UTF-8 text
- `Bool` - true or false
- `[]T` - Array of T
- `{}` - Record type

### Type Annotation Syntax

- `::` - Type annotation (e.g., `x :: Num`)
- `->` - Return type (e.g., `-> String`)
- `{}` - Generics (e.g., `Result{T, E}`)

### Sum Types

```quilon
Result{T, E} = Ok T | Err E
Option{T} = Some T | None
```

### Records

```quilon
User = { name :: String, age :: Num, active :: Bool }

user = { name = "Alice", age = 30, active = true }
```

## Control Flow

### If Expression

```quilon
result = condition ? true_value : false_value
```

### Pattern Matching

```quilon
handle = result :: Result{T, E} => result ?
  | Ok value  => value
  | Err error => <
      log.error "Failed"
      panic error
    >
```

### Loops

```quilon
~ For loop (auto-parallelizes if safe)
sum = items |> fold 0 (acc, x => acc + x)

~ While loop
mut i = 0
while i < 10 <
  print i
  i = i + 1
>
```

## Operators

### Arithmetic

- `+` Addition
- `-` Subtraction
- `*` Multiplication
- `/` Division
- `%` Modulo

### Comparison

- `==` Equal
- `!=` Not equal
- `<` Less than
- `>` Greater than
- `<=` Less or equal
- `>=` Greater or equal

### Logical

- `&&` And
- `||` Or
- `!` Not

### Pipeline

- `|>` Pipeline (passes left value to right function)

```quilon
result = data
  |> filter .active
  |> map transform
  |> collect
```

### Field Access

- `.field` - Access field
- `.method` - Call method

Shorthand in pipelines:

```quilon
users |> filter .active  ~ equivalent to: users |> filter (u => u.active)
```

## Strings

All strings are UTF-8. Use `<var>` for interpolation:

```quilon
name = "Alice"
message = "Hello, <name>!"  ~ "Hello, Alice!"
```

## Blocks

Multi-line blocks use `<` to open and `>` to close:

```quilon
compute = input :: Num => <
  temp = input * 2
  result = temp + 10
  result  ~ last expression is return value
>
```

Blocks are expressions - they return their last value.

## Immutability

By default, all bindings are deeply immutable:

```quilon
user = { name = "Alice", age = 30 }
user.name = "Bob"  ~ ERROR: cannot mutate immutable binding
```

Use `mut` for mutable bindings:

```quilon
mut user = { name = "Alice", age = 30 }
user.name = "Bob"  ~ OK
```

## Auto-Parallelization

The compiler auto-parallelizes safe operations:

```quilon
~ This runs in parallel if fetch is pure and ids is immutable
results = ids |> map fetch |> collect
```

Force sequential execution:

```quilon
@sequential
results = items |> map process_in_order
```

## File Extension

Quilon source files use `.ql` extension.
