//! Compile-speed benchmark: how long each front-end phase takes on generated programs.
//!
//! Run it with `cargo bench`. There is no pass/fail here — it prints a table, and CI
//! publishes that table so the numbers can be watched over time. A regression shows up
//! as a column growing across commits, which is the thing nothing could see before.
//!
//! The corpora are generated rather than committed so their size is a number in one
//! place, and so they stay honest: each one stresses a different part of the pipeline
//! (sheer item count, expression depth, overload-set width, corelib imports).
//!
//! Phases are timed separately because they scale differently, and a total alone hides
//! which one moved. `link` resolves `<<` imports; it is zero for corpora with none.

use std::fmt::Write as _;
use std::time::{Duration, Instant};

use inkwell::context::Context;
use quilon::codegen::CodeGenerator;
use quilon::lexer::Lexer;
use quilon::parser;
use quilon::typechecker::TypeChecker;

/// How many times each corpus is compiled. The reported number is the mean; a handful
/// of runs is enough to damp scheduler noise without making the suite slow enough that
/// people skip it.
const RUNS: u32 = 5;

/// Where the corpora live, relative to the crate root. The files in here are the
/// benchmark's input: what runs is the committed bytes, so every run — yours, mine,
/// CI's, and one a year from now — compiles exactly the same programs.
const CORPUS_DIR: &str = "benches/corpus";

/// The corpora, in table order: file stem, and what the file is shaped to stress.
const CORPORA: &[(&str, &str)] = &[
    ("flat", "4000 top-level functions"),
    ("deep", "300 functions, each nested 100 deep"),
    ("wide_overloads", "300-member overload set"),
    ("corelib", "imports core.io/test/cli"),
    ("many_modules", "50 imported files"),
    ("interpolation", "600 interpolated literals"),
    ("sum_matches", "40 variants, 120 exhaustive matches"),
    ("records", "30 record types of 20 fields, used 4x each"),
    ("call_sites", "2000 assertions, deep in a long file"),
];

fn main() {
    if std::env::args().any(|a| a == "--regen") {
        regenerate();
        return;
    }

    println!("Compile-speed benchmark — mean of {RUNS} runs, milliseconds\n");
    println!("| corpus | shape | bytes | lex | parse | link | check | codegen | total |");
    println!("|---|---|--:|--:|--:|--:|--:|--:|--:|");
    for (stem, shape) in CORPORA {
        let corpus = Corpus::read(stem, shape);
        let t = corpus.measure();
        println!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | **{}** |",
            corpus.name,
            corpus.shape,
            corpus.source.len(),
            ms(t.lex),
            ms(t.parse),
            ms(t.link),
            ms(t.check),
            ms(t.codegen),
            ms(t.total()),
        );
    }
    if let Some(kb) = peak_rss_kb() {
        // One figure for the whole run rather than a column: these corpora are compiled
        // in one process, so the kernel's high-water mark is shared between them. It is
        // dominated by the largest, which is the number the memory work needs anyway.
        println!("Peak RSS for the whole run: {:.1} MB", kb as f64 / 1024.0);
    }
    // A trailing blank line closes the table for whatever reads it back out of the log.
    println!();
}

/// The process's peak resident set, as the kernel recorded it. `None` where the OS does
/// not report one this way.
fn peak_rss_kb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        let rest = line.strip_prefix("VmHWM:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// Rewrite `benches/corpus/` from the generators below — `cargo bench -- --regen`.
///
/// The generators are kept for this one purpose: producing a corpus of a different size
/// is a deliberate act whose result lands in git as a reviewable diff, not something
/// that happens quietly on the next run because a constant moved. Changing a corpus
/// breaks comparability with every number recorded before it, so it should be visible.
fn regenerate() {
    let dir = corpus_dir();
    for (stem, source) in [
        ("flat", flat_program(4000)),
        ("deep", deep_program(300, 100)),
        ("wide_overloads", overload_program(300)),
        ("corelib", corelib_program()),
        ("interpolation", interpolation_program(600)),
        ("sum_matches", sum_match_program(40, 120)),
        ("records", record_program(30, 20, 4)),
        ("call_sites", call_site_program(2000)),
    ] {
        let path = dir.join(format!("{stem}.ql"));
        std::fs::write(&path, source).unwrap_or_else(|e| panic!("writing {path:?}: {e}"));
        println!("wrote {}", path.display());
    }

    let modules = dir.join("many_modules");
    std::fs::create_dir_all(&modules).unwrap_or_else(|e| panic!("creating {modules:?}: {e}"));
    for (name, source) in many_modules_program(50) {
        let path = modules.join(name);
        std::fs::write(&path, source).unwrap_or_else(|e| panic!("writing {path:?}: {e}"));
        println!("wrote {}", path.display());
    }
}

fn corpus_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS_DIR)
}

/// One corpus: the committed source, with a label for the table.
struct Corpus {
    name: &'static str,
    shape: &'static str,
    source: String,
    /// The directory a `<< "path"` import resolves against — the corpus's own, so a
    /// multi-file corpus finds its siblings.
    dir: std::path::PathBuf,
}

#[derive(Default)]
struct Timing {
    lex: Duration,
    parse: Duration,
    link: Duration,
    check: Duration,
    codegen: Duration,
}

impl Timing {
    fn total(&self) -> Duration {
        self.lex + self.parse + self.link + self.check + self.codegen
    }
}

impl Corpus {
    /// Read a committed corpus. A missing file means someone deleted an input rather
    /// than that the benchmark should quietly measure something else, so it is fatal.
    fn read(name: &'static str, shape: &'static str) -> Self {
        let path = match name {
            // A multi-file corpus is a directory; its root imports the siblings beside it.
            "many_modules" => corpus_dir().join("many_modules").join("root.ql"),
            _ => corpus_dir().join(format!("{name}.ql")),
        };
        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!("reading corpus {path:?}: {e} — run `cargo bench -- --regen` to rebuild it")
        });
        let dir = path.parent().unwrap_or(&path).to_path_buf();
        Self {
            name,
            shape,
            source,
            dir,
        }
    }

    /// Compile the corpus `RUNS` times, accumulating each phase, and return the means.
    /// Every phase feeds the next exactly as the real driver wires them, so what is
    /// measured is the work a `quilon build` actually does — including handing codegen
    /// the table the checker just produced, rather than making it re-derive one.
    fn measure(&self) -> Timing {
        let mut total = Timing::default();
        for _ in 0..RUNS {
            let start = Instant::now();
            let tokens = Lexer::tokenize(&self.source).expect("benchmark corpus must lex");
            total.lex += start.elapsed();

            let start = Instant::now();
            let program = parser::parse(&tokens).expect("benchmark corpus must parse");
            total.parse += start.elapsed();

            let start = Instant::now();
            let (program, mut sources) = quilon::modules::link(program, &self.dir)
                .expect("benchmark corpus must resolve its imports");
            total.link += start.elapsed();
            // The corpus's own text, under the name the table shows. Codegen resolves a
            // call site through this map, so WITHOUT it every `Site` a corpus asks for
            // would take the "unknown location" path and the work would not be measured.
            sources.set_root(format!("{}.ql", self.name), self.source.clone());
            let sources = std::rc::Rc::new(sources);

            let start = Instant::now();
            let table = TypeChecker::new()
                .check_program(&program)
                .expect("benchmark corpus must type-check");
            total.check += start.elapsed();

            let start = Instant::now();
            let context = Context::create();
            let mut codegen = CodeGenerator::new(&context, "bench");
            codegen.set_type_table(table);
            codegen.set_source_map(std::rc::Rc::clone(&sources));
            codegen
                .generate(&program)
                .expect("benchmark corpus must compile");
            total.codegen += start.elapsed();
        }
        Timing {
            lex: total.lex / RUNS,
            parse: total.parse / RUNS,
            link: total.link / RUNS,
            check: total.check / RUNS,
            codegen: total.codegen / RUNS,
        }
    }
}

fn ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

/// Many small top-level functions: scales item count, so it moves with anything that is
/// per-declaration (registration, symbol mangling, emitting a function).
fn flat_program(count: usize) -> String {
    let mut src = String::new();
    for i in 0..count {
        let _ = writeln!(src, "f{i} = (x :: Num) -> Num => x * {i} + 1");
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for i in 0..count {
        let _ = writeln!(src, "  n{i} = f{i}(1)");
    }
    let _ = writeln!(src, "  n0\n>");
    src
}

/// Deeply parenthesized expressions: scales recursion depth rather than item count, so
/// it moves with anything per-level in the descent (the parser's precedence chain, the
/// checker's walk, expression lowering). Depth stays under the parser's nesting ceiling,
/// so the corpus gets its size from repeating the expression rather than nesting further.
fn deep_program(functions: usize, depth: usize) -> String {
    let mut expr = String::from("1");
    for i in 0..depth {
        expr = format!("({expr} + {i})");
    }
    let mut src = String::new();
    for i in 0..functions {
        let _ = writeln!(src, "d{i} = () -> Num => {expr}");
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for i in 0..functions {
        let _ = writeln!(src, "  n{i} = d{i}()");
    }
    let _ = writeln!(src, "  n0\n>");
    src
}

/// One name with many members, and a call to each: scales overload-set width, so it
/// moves with resolution cost — which is a scan per call site.
fn overload_program(members: usize) -> String {
    let mut src = String::new();
    for i in 0..members {
        let _ = writeln!(src, "T{i} = {{ v :: Num }}");
    }
    for i in 0..members {
        let _ = writeln!(src, "pick = (a :: T{i}) -> Num => a.v");
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for i in 0..members {
        let _ = writeln!(src, "  n{i} = pick(T{i} {{ v = {i} }})");
    }
    let _ = writeln!(src, "  n0");
    let _ = writeln!(src, ">");
    src
}

/// A tiny program that imports the core library: almost all of its cost is the corelib
/// itself, which is checked and emitted whole whether or not the program uses it.
/// Deliberately leaves the imported library unused apart from one assertion: this is the
/// real shape of a small program that imports `core.*`, and the row where emission-side
/// pruning shows up. The other corpora reach every function they define, so they keep
/// measuring codegen; this one measures what an import costs.
fn corelib_program() -> String {
    "<< core.io\n<< core.test\n<< core.cli\n\n^ = () -> $ => assert(1 + 1 == 2)\n".to_string()
}

/// Assertions far down a long file: every one is a call whose trailing `Site` the compiler
/// fills in, which means resolving a byte offset to a line, column, and source line.
///
/// The padding above them is the point. Resolving a position used to walk the file from
/// offset 0, so the cost grew with each call's DISTANCE into the file — the same 2000
/// assertions placed after a long prologue cost an order of magnitude more than at the top.
/// A corpus that put its calls near line 1 would have measured almost none of it.
fn call_site_program(count: usize) -> String {
    let mut out = String::from("<< core.test\n\n");
    for line in 0..count {
        out.push_str(&format!(
            "~ padding, so the assertions below sit deep in the file (line {line})\n"
        ));
    }
    out.push_str("\n^ = () -> $ => <\n");
    for n in 0..count {
        out.push_str(&format!("  assertEq({n} + 1, {})\n", n + 1));
    }
    out.push_str(">\n");
    out
}

/// Many small imported files: scales the module system — resolution, per-file span
/// plumbing, and checking every export whether or not the root uses it.
///
/// The modules are named after plausible subjects and their exports after what each
/// function would do, so a failure that names one (`cannot read module "pricing.ql"`,
/// or a span inside `geometry_scale`) says where to look. The bodies are arithmetic
/// stand-ins: the corpus measures the module machinery, not the code inside.
fn many_modules_program(count: usize) -> Vec<(String, String)> {
    const SUBJECTS: &[&str] = &[
        "arithmetic",
        "geometry",
        "statistics",
        "strings",
        "parsing",
        "validation",
        "formatting",
        "currency",
        "dates",
        "durations",
        "angles",
        "vectors",
        "matrices",
        "physics",
        "chemistry",
        "astronomy",
        "navigation",
        "mapping",
        "routing",
        "scheduling",
        "billing",
        "inventory",
        "pricing",
        "shipping",
        "ordering",
        "catalog",
        "payments",
        "accounting",
        "budgeting",
        "forecasting",
        "sampling",
        "ranking",
        "scoring",
        "matching",
        "filtering",
        "sorting",
        "hashing",
        "encoding",
        "compression",
        "checksums",
        "geometry3d",
        "colour",
        "audio",
        "imaging",
        "telemetry",
        "logging",
        "caching",
        "batching",
        "throttling",
        "retrying",
    ];
    const OPERATIONS: &[&str] = &[
        "offset", "scale", "clamp", "round", "wrap", "snap", "bias", "damp", "boost", "trim",
    ];

    let subject_of = |i: usize| {
        // Past the list, keep names unique and still readable rather than wrapping.
        match i / SUBJECTS.len() {
            0 => SUBJECTS[i].to_string(),
            n => format!("{}{}", SUBJECTS[i % SUBJECTS.len()], n + 1),
        }
    };

    let mut files = Vec::new();
    for i in 0..count {
        let subject = subject_of(i);
        let mut module = String::new();
        for (step, operation) in OPERATIONS.iter().enumerate() {
            let _ = writeln!(
                module,
                ">> {subject}_{operation} = (x :: Num) -> Num => x + {step}"
            );
        }
        files.push((format!("{subject}.ql"), module));
    }

    let mut root = String::new();
    for i in 0..count {
        let _ = writeln!(root, "<< \"{}.ql\"", subject_of(i));
    }
    let _ = writeln!(root, "\n^ = () -> Num => <");
    for i in 0..count {
        for operation in OPERATIONS {
            let _ = writeln!(root, "  {}_{operation}(1)", subject_of(i));
        }
    }
    let _ = writeln!(root, "  0\n>");
    files.push(("root.ql".to_string(), root));
    files
}

/// Interpolated literals: every hole is an expression to check and a render call to
/// emit, so this scales the interpolation path rather than plain text.
fn interpolation_program(count: usize) -> String {
    let mut src = String::from("<< core.io\n\n");
    for i in 0..count {
        let _ = writeln!(
            src,
            "s{i} = (n :: Num, t :: Text) -> Text => \"item `n` of `t` at `n * {i}` end\""
        );
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for i in 0..count {
        let _ = writeln!(src, "  n{i} = s{i}(1, \"a\").size");
    }
    let _ = writeln!(src, "  n0\n>");
    src
}

/// A wide sum type matched exhaustively many times: scales variant registration, arm
/// checking, exhaustiveness, and the tag dispatch codegen emits per match.
/// Record-heavy code: `types` named record types, each with `fields` fields and three
/// methods, each constructed / spread / read by `users` functions. Named record types are
/// the one shape whose *declaration* is carried around by the checker — a function that
/// takes one has the whole field list in its parameter type — so this is where the cost
/// of moving type information through the front end shows up, and the other corpora
/// (which barely use records) do not see it at all.
fn record_program(types: usize, fields: usize, users: usize) -> String {
    let declared = (0..fields)
        .map(|f| format!("  f{f} :: Num,"))
        .collect::<Vec<_>>()
        .join("\n");
    let initializer = (0..fields)
        .map(|f| format!("f{f} = {f}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut src = String::new();
    for t in 0..types {
        let _ = writeln!(
            src,
            "R{t} = {{\n{declared}\n  first = => it.f0,\n  scaled = k => it.f0 * k + it.f1,\n  total = => it.f0 + it.f1 + it.f2\n}}\n"
        );
    }
    for t in 0..types {
        for u in 0..users {
            let _ = writeln!(
                src,
                "read{t}_{u} = (r :: R{t}) -> Num => r.f0 + r.f1 + r.scaled(2) + r.total()"
            );
            let _ = writeln!(
                src,
                "build{t}_{u} = (k :: Num) -> Num => <\n  r = R{t} {{ {initializer} }}\n  s = R{t} {{ <-r, f0 = k }}\n  read{t}_{u}(r) + read{t}_{u}(s) + s.first()\n>\n"
            );
        }
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for t in 0..types {
        for u in 0..users {
            let _ = writeln!(src, "  n{t}_{u} = build{t}_{u}({u})");
        }
    }
    let _ = writeln!(src, "  n0_0\n>");
    src
}

fn sum_match_program(variants: usize, matches: usize) -> String {
    let alternatives = (0..variants)
        .map(|i| format!("V{i}(Num)"))
        .collect::<Vec<_>>()
        .join(" / ");
    let mut src = format!("Wide = {alternatives}\n\n");
    for m in 0..matches {
        let arms = (0..variants)
            .map(|i| format!("  | V{i}(n) => n + {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = writeln!(src, "pick{m} = (w :: Wide) -> Num => w ?\n{arms}\n");
    }
    let _ = writeln!(src, "^ = () -> Num => <");
    for m in 0..matches {
        let _ = writeln!(src, "  n{m} = pick{m}(V0(1))");
    }
    let _ = writeln!(src, "  n0\n>");
    src
}
