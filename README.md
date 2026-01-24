# Quilon

**A fast, statically-typed web programming language**

Quilon (`.ql`) is a systems-level language designed specifically for high-performance web applications. It combines the safety and speed of compiled languages with the ergonomics of modern web development.

## Key Features

- **Implicit Parallelism**: Write sequential code, get parallel execution automatically
- **Static Strong Typing**: With a simple unified `Num` type and full type inference
- **Deep Immutability**: Immutable by default with no interior mutation escapes
- **Native Compilation**: Compiles to native code via LLVM for maximum performance
- **Unique Syntax**: No curly braces, no indentation rules, symbolic delimiters
- **UTF-8 First**: All strings are UTF-8 by default
- **No Function Coloring**: No async/await - I/O is non-blocking under the hood

## Syntax Overview

```quilon
~ Variables - no keywords needed
name = "Quilon"
port = 3000

~ Functions are arrow assignments
greet = name :: String => "Hello, <name>!"

~ Multi-line blocks use < >
fetch_user = id :: Num => <
  http.get "https://api.example.com/users/<id>"
    |> json.parse{User}
>

~ Auto-parallelized pipelines
process = data :: []Record => <
  data
    |> filter .active
    |> map transform
    |> collect
>

~ Pattern matching with ?
handle = result :: Result{T, E} => result ?
  | Ok value  => value
  | Err error => panic error
```

## Core Types

- `Num` - All numbers (int/float inferred by compiler)
- `String` - UTF-8 text
- `Bool` - true/false
- `[]T` - Arrays
- `{}` - Records/objects

## Building from Source

```bash
cargo build --release
```

## Running Quilon Programs

```bash
quilon run program.ql
```

## Project Status

🚧 **Early Development** - The compiler is currently being built.

## Design Principles

1. **Simplicity**: No unnecessary keywords or syntax noise
2. **Performance**: Native speed through LLVM compilation
3. **Safety**: Deep immutability enables fearless parallelism
4. **Ergonomics**: Write simple code that runs fast

## License

GPL-2.0 License

## Learn More

See the [docs](./docs) directory for detailed documentation.
