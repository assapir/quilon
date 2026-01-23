// Example: Demonstrate Quilon lexer

use quilon::lexer::Lexer;

fn main() {
    let source = r#"
~ Fibonacci in Quilon
fib = n :: Num => n ?
  | 0 => 0
  | 1 => 1
  | n => fib (n - 1) + fib (n - 2)

main = => <
  result = fib 10
  print "Fibonacci(10) = <result>"
>
"#;

    println!("Source code:");
    println!("{}", source);
    println!("\n{}", "=".repeat(60));
    println!("Tokens:");
    println!("{}", "=".repeat(60));

    match Lexer::tokenize(source) {
        Ok(tokens) => {
            for (i, token) in tokens.iter().enumerate() {
                if token.kind != quilon::lexer::TokenKind::Eof {
                    println!("{:3}: {:20} {} at {}", 
                        i, 
                        format!("{:?}", token.kind),
                        token.text,
                        token.span
                    );
                }
            }
            println!("\nTotal tokens: {}", tokens.len() - 1); // Exclude EOF
        }
        Err(e) => {
            eprintln!("Lexer error: {}", e);
        }
    }
}
