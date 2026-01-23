# Quilon Language Reference

**Version:** 0.1.0 (Early Development)  
**Philosophy:** Symbol-based syntax, no keywords, implicit parallelism

Quilon is a statically-typed programming language that compiles to native code via LLVM. It features a unique syntax with no traditional keywords, using symbols instead.

---

## Table of Contents

1. [Syntax Overview](#syntax-overview)
2. [Types](#types)
3. [Variables](#variables)
4. [Functions](#functions)
5. [Expressions](#expressions)
6. [Pattern Matching](#pattern-matching)
7. [Entry Point](#entry-point)
8. [Examples](#examples)

---

## Syntax Overview

### Core Principles

- **No keywords** - Uses symbols instead (`>>`, `<>`, `::`, `=>`, etc.)
- **Everything is an expression** - Even blocks return values
- **Statically typed** - With type inference
- **Immutable by default** - Use `mut` for mutability
- **Comments** - Use `~` for single-line comments

### Symbols Reference

| Symbol | Meaning | Example |
|--------|---------|---------|
| `=` | Assignment/binding | `x = 42` |
| `::` | Type annotation | `x :: Num` |
| `=>` | Function body | `f = x => x + 1` |
| `->` | Return type | `f = x -> Num => x` |
| `< >` | Block delimiters | `< stmt1 stmt2 >` |
| `>>` | Entry point (main) | `>> = () -> Num => 0` |
| `?` | Pattern match | `x ? \| 1 => "one"` |
| `\|` | Pattern arm separator | `\| pattern => body` |
| `_` | Wildcard pattern | `\| _ => default` |
| `~` | Comment | `~ This is a comment` |

---

## Types

### Primitive Types

#### Num
All numbers (integers and floats) are unified as `Num`. The compiler represents them as f64 internally.

```quilon
x = 42          ~ Integer
y = 3.14        ~ Float
z = x + y       ~ Mixed arithmetic works
```

**Example:** [examples/simple.ql](examples/simple.ql)

#### String
UTF-8 text strings.

```quilon
name = "Alice"
greeting = "Hello, World!"
```

**Example:** [examples/string_test.ql](examples/string_test.ql), [examples/hello_world.ql](examples/hello_world.ql)

#### Bool
Boolean values: `true` or `false`.

```quilon
flag = true
done = false
```

### Composite Types

#### Arrays
Homogeneous collections with `[]T` syntax.

```quilon
numbers = [1, 2, 3, 4, 5]
names = ["Alice", "Bob", "Charlie"]
```

**Example:** [examples/simple.ql](examples/simple.ql)

#### Records
Structs/objects with named fields using `{ }` syntax.

```quilon
user = { name = "Alice", age = 30 }
point = { x = 10.5, y = 20.3 }
```

Field access uses `.` notation:
```quilon
userName = user.name    ~ "Alice"
```

#### Function Types
Functions are first-class values.

```quilon
~ Type signature: (Num, Num) -> Num
add = (a :: Num, b :: Num) -> Num => a + b
```

### Sum Types

#### Result{T}
The built-in sum type for computations that may succeed or fail.

**Constructors:**
- `OK(value)` - Success case (not yet implemented in codegen)
- `NotOK` - Failure/absence case (not yet implemented in codegen)

```quilon
~ Pattern matching on Result (currently only matches work, not constructors)
result = value ?
  | OK(x) => x * 2
  | NotOK => 0
```

**Note:** Result replaces the traditional Option and Result types found in other languages. Use `NotOK` for both "no value" and "error" cases.

---

## Variables

### Immutable Variables (Default)

```quilon
x = 42
name = "Alice"
```

Once bound, immutable variables cannot be reassigned.

### Mutable Variables

Use `mut` prefix:

```quilon
mut counter = 0
counter = counter + 1    ~ OK: counter is mutable
```

### Type Annotations

Explicit types with `::`:

```quilon
x :: Num = 42
name :: String = "Alice"
age :: Num = 30
```

Type inference usually makes annotations optional:

```quilon
x = 42          ~ Type inferred as Num
name = "Alice"  ~ Type inferred as String
```

---

## Functions

### Function Declaration Syntax

Quilon supports multiple function syntaxes:

#### No parameters
```quilon
greet = => "Hello!"
```

#### Single parameter (no parentheses needed)
```quilon
double = x => x * 2
```

#### Multiple parameters
```quilon
add = (a, b) => a + b
```

#### With type annotations
```quilon
add = (a :: Num, b :: Num) => a + b
```

#### With return type
```quilon
add = (a :: Num, b :: Num) -> Num => a + b
```

**Examples:** 
- [examples/factorial.ql](examples/factorial.ql) - Recursive function
- [examples/fibonacci.ql](examples/fibonacci.ql) - Double recursion
- [examples/math.ql](examples/math.ql) - Function composition

### Multi-statement Functions

Use `< >` blocks for multiple statements:

```quilon
compute = x => <
  doubled = x * 2
  squared = doubled * doubled
  squared
>
```

**Example:** [examples/factorial.ql](examples/factorial.ql)

### Recursion

Functions can call themselves. The function name is in scope during its own body.

```quilon
factorial = n -> Num => <
  result = n == 0 ?
    1
  :
    n * factorial(n - 1)
  
  result
>
```

**Examples:**
- [examples/factorial.ql](examples/factorial.ql) - `factorial(5) = 120`
- [examples/fibonacci.ql](examples/fibonacci.ql) - `fibonacci(10) = 55`

### Function Calls

```quilon
result = add(5, 7)           ~ Call with arguments
greeting = greet()           ~ Call with no arguments
value = compute(42)          ~ Call single-argument function
```

---

## Expressions

### Literals

```quilon
42              ~ Number (Num)
3.14159         ~ Number (Num)
"Hello"         ~ String
true            ~ Bool
false           ~ Bool
```

### Arithmetic Operations

```quilon
x + y           ~ Addition
x - y           ~ Subtraction
x * y           ~ Multiplication
x / y           ~ Division
-x              ~ Negation
```

**Example:** [examples/math.ql](examples/math.ql)

### Comparison Operations

```quilon
x == y          ~ Equal
x != y          ~ Not equal
x < y           ~ Less than
x <= y          ~ Less than or equal
x > y           ~ Greater than
x >= y          ~ Greater than or equal
```

**Example:** [examples/factorial.ql](examples/factorial.ql) - Uses `==` in recursion base case

### Logical Operations

**Not yet implemented in codegen**

```quilon
!flag           ~ Logical NOT
x && y          ~ Logical AND (not yet in codegen)
x || y          ~ Logical OR (not yet in codegen)
```

### Conditional Expressions (Ternary)

Use `?` and `:` for inline conditionals:

```quilon
result = condition ? thenValue : elseValue

~ Example:
abs = x => x < 0 ? -x : x
```

**Example:** [examples/factorial.ql](examples/factorial.ql)

### If Expressions

Quilon uses ternary syntax (no `if` keyword):

```quilon
value = x > 0 ? "positive" : "non-positive"
```

Both branches must return the same type.

### Blocks

Blocks are expressions that return their last value:

```quilon
result = <
  x = 10
  y = 20
  x + y          ~ Returns 30
>
```

**Example:** Most examples use blocks, see [examples/fibonacci.ql](examples/fibonacci.ql)

### Arrays

```quilon
nums = [1, 2, 3, 4, 5]
~ Element access not yet implemented
```

### Records

```quilon
person = { name = "Alice", age = 30 }
personName = person.name    ~ Field access
```

---

## Pattern Matching

Pattern matching uses the `?` operator with `|` separated arms.

### Syntax

```quilon
expression ?
  | pattern1 => body1
  | pattern2 => body2
  | _ => defaultBody
```

### Pattern Types

#### Number Patterns

Match specific numeric values:

```quilon
result = value ?
  | 0 => "zero"
  | 1 => "one"
  | 5 => "five"
  | _ => "other"
```

**Example:** [examples/option.ql](examples/option.ql)

#### Wildcard Pattern

`_` matches anything:

```quilon
result = value ?
  | 42 => "the answer"
  | _ => "something else"
```

**Example:** [examples/pattern_wildcard.ql](examples/pattern_wildcard.ql)

#### Identifier Pattern

Binds the value to a name:

```quilon
result = value ?
  | x => x * 2    ~ x is bound to value
```

#### Constructor Patterns

Match and destructure sum type constructors:

```quilon
result = maybeValue ?
  | OK(x) => x
  | NotOK => 0
```

**Note:** Constructor value creation (e.g., `OK(42)`) not yet implemented in codegen.

**Example:** [examples/result_test.ql](examples/result_test.ql)

### Exhaustiveness

The type checker verifies that pattern matches are exhaustive. Use `_` to ensure all cases are covered.

---

## Entry Point

Every Quilon program must define an entry point using the `>>` symbol.

### Syntax

```quilon
>> = () -> Num => <
  ~ Your program here
  0    ~ Exit code
>
```

### Rules

- The `>>` function is the program entry point (like `main` in C)
- Currently must take no parameters: `() -> Num`
- Must return `Num` - becomes the program's exit code
- The compiler auto-generates a C-compatible `main()` wrapper

**Future:** Will support `(args, envp)` parameters for command-line arguments and environment variables.

### Exit Codes

The value returned from `>>` becomes the program's exit code:

```quilon
>> = () -> Num => 0           ~ Exit with success (0)
>> = () -> Num => 42          ~ Exit with code 42
>> = () -> Num => 1           ~ Exit with error (1)
```

**Examples:**
- [examples/hello_world.ql](examples/hello_world.ql) - `exit 42`
- [examples/factorial.ql](examples/factorial.ql) - `exit 120`
- [examples/fibonacci.ql](examples/fibonacci.ql) - `exit 55`

---

## Examples

### Hello World

**File:** [examples/hello_world.ql](examples/hello_world.ql)

```quilon
>> = () -> Num => 42
```

Exit code: 42

---

### Simple Arithmetic

**File:** [examples/simple.ql](examples/simple.ql)

```quilon
>> = () -> Num => <
  a = 5
  b = 7
  a + b
>
```

Exit code: 12

---

### String Literals

**File:** [examples/string_test.ql](examples/string_test.ql)

```quilon
greet = name :: String -> String => "Hello"

>> = () -> Num => <
  msg = greet("World")
  42
>
```

Exit code: 42

---

### Function Composition

**File:** [examples/math.ql](examples/math.ql)

```quilon
square = x :: Num -> Num => x * x

>> = () -> Num => <
  a = square(3)    ~ 9
  b = square(4)    ~ 16
  a + b            ~ 25
>
```

Exit code: 25 (Pythagorean theorem: 3² + 4² = 5²)

---

### Recursion - Factorial

**File:** [examples/factorial.ql](examples/factorial.ql)

```quilon
factorial = n -> Num => <
  result = n == 0 ?
    1
  :
    n * factorial(n - 1)
  
  result
>

>> = () -> Num => factorial(5)
```

Exit code: 120 (5! = 120)

---

### Double Recursion - Fibonacci

**File:** [examples/fibonacci.ql](examples/fibonacci.ql)

```quilon
fibonacci = n -> Num => <
  result = n <= 1 ?
    n
  :
    fibonacci(n - 1) + fibonacci(n - 2)
  
  result
>

>> = () -> Num => fibonacci(10)
```

Exit code: 55 (10th Fibonacci number)

---

### Pattern Matching - Numbers

**File:** [examples/option.ql](examples/option.ql)

```quilon
>> = () -> Num => <
  value = 5
  
  result = value ?
    | 5 => 50
    | 3 => 30
    | _ => 0
  
  result
>
```

Exit code: 50 (matches first pattern)

---

### Pattern Matching - Wildcard

**File:** [examples/pattern_wildcard.ql](examples/pattern_wildcard.ql)

```quilon
>> = () -> Num => <
  value = 7
  
  result = value ?
    | 5 => 50
    | 3 => 30
    | _ => 99
  
  result
>
```

Exit code: 99 (falls through to wildcard)

---

## Compilation

### Compile a Program

```bash
# Compile to LLVM IR
cargo run -- compile examples/hello_world.ql

# Generate object file
llc -filetype=obj examples/hello_world.ll

# Link to executable
gcc examples/hello_world.o -o examples/hello_world

# Run
./examples/hello_world
echo "Exit code: $?"
```

### Type Check Only

```bash
cargo run -- check examples/hello_world.ql
```

---

## Language Features Summary

### ✅ Implemented

- [x] Lexer with symbol-based tokens
- [x] Parser with 17 precedence levels
- [x] Type checker with inference
- [x] LLVM code generation
- [x] Variables (immutable and mutable)
- [x] Functions with recursion
- [x] Arithmetic operations (+, -, *, /)
- [x] Comparison operations (==, !=, <, <=, >, >=)
- [x] Conditional expressions (ternary)
- [x] Blocks
- [x] Arrays (literals only)
- [x] Records with field access
- [x] Pattern matching (numbers, wildcards, identifiers)
- [x] Entry point (`>>`)
- [x] Result{T} sum type (type checking only)
- [x] Native compilation to executables

### 🚧 Partially Implemented

- [ ] Pattern matching on constructors (type checks but doesn't codegen discriminants)
- [ ] Sum type constructors (OK/NotOK parsing works, codegen needed)

### ❌ Not Yet Implemented

- [ ] Logical operators in codegen (&&, ||)
- [ ] Array element access
- [ ] Generic types (proper polymorphism)
- [ ] Closures
- [ ] Module system / imports
- [ ] String operations (concatenation, interpolation)
- [ ] Standard library
- [ ] Command-line arguments / environment variables in `>>`
- [ ] Pipeline operator (`|>`)
- [ ] Custom sum types (beyond Result)

---

## Type System Notes

### Unified Result Type

Quilon uses a single `Result{T}` type instead of separate Option and Result types:

- **OK(value)** - For success cases (replaces Some/Ok from other languages)
- **NotOK** - For failure/absence cases (replaces None/Err)

This provides a simpler mental model: "might not work" always uses Result.

### Type Inference

The type checker infers types where possible:

```quilon
x = 42              ~ Inferred as Num
f = x => x + 1      ~ Inferred as Num -> Num
```

But return types for recursive functions must be annotated:

```quilon
factorial = n -> Num => <    ~ -> Num required for recursion
  ~ ...
>
```

---

## Compiler Architecture

1. **Lexer** (Logos) - Source → Tokens
2. **Parser** (Hand-written recursive descent) - Tokens → AST
3. **Type Checker** - AST → Typed AST
4. **Code Generator** (Inkwell/LLVM) - Typed AST → LLVM IR
5. **LLVM** - LLVM IR → Native binary

---

## Error Messages

The compiler provides basic error messages for:

- Type mismatches
- Undefined variables
- Wrong number of function arguments
- Non-exhaustive pattern matches
- Missing entry point (`>>`)

**Future:** Source context and better diagnostics planned.

---

## Contributing

Quilon is in early development. Core language features are stabilizing, but the syntax and semantics may still evolve.

See [plan.md](/.copilot/session-state/f77dc225-c857-4b67-a2ce-00b4479ea48c/plan.md) for the development roadmap.

---

## License

[To be determined]

---

**End of Language Reference**
