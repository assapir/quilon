// Example: Demonstrate Quilon parser

use quilon::lexer::Lexer;
use quilon::parser::ast_parser::parse;

fn main() {
    let examples = vec![
        ("Simple variable", "x = 42"),
        ("Mutable variable", "mut counter = 0"),
        ("With type", "port :: Num = 3000"),
        ("Arithmetic", "result = 2 + 3 * 4"),
        ("Pipeline", "result = data |> filter |> collect"),
        ("Function call", "sum = add(1, 2)"),
        ("Field access", "name = user.name"),
        ("Ternary", "abs = x >= 0 ? x : -x"),
        ("Array", "nums = [1, 2, 3]"),
        ("Complex", "result = (a + b) * c |> process"),
        ("Simple function", "add = (a, b) => a + b"),
        (
            "Typed function",
            "add = (a :: Num, b :: Num) -> Num => a + b",
        ),
        ("No-param function", "main = => 42"),
        ("Single-param function", "greet = name => \"Hello\""),
        ("Function with block", "test = => < x = 1 x >"),
        ("Pattern match", "result = x ? | Some(v) => v | None => 0"),
        ("Record literal", "user = { name = \"Alice\", age = 30 }"),
    ];

    println!("Quilon Parser Demo");
    println!("{}", "=".repeat(60));

    for (desc, source) in examples {
        println!("\n{}: {}", desc, source);

        match Lexer::tokenize(source) {
            Ok(tokens) => match parse(&tokens) {
                Ok(program) => {
                    println!("✓ Parsed successfully");
                    println!("  Items: {}", program.items.len());

                    if let Some(quilon::ast::Item::VarDecl(decl)) = program.items.first() {
                        println!("  Variable: {}", decl.name);
                        println!("  Mutable: {}", decl.mutable);
                        println!("  Type: {:?}", decl.type_annotation);
                    }
                }
                Err(e) => {
                    println!("✗ Parse error: {}", e);
                }
            },
            Err(e) => {
                println!("✗ Lexer error: {}", e);
            }
        }
    }
}
