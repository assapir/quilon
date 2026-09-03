//! The error-code registry: every diagnostic the compiler or a compiled program can raise,
//! numbered `QN000` upward — the first digit names the pipeline family (`0` lexer, `1`
//! parser, `2` module resolution and linking, `3` type checker, `4` codegen and build, `5`
//! runtime, `6` CLI and usage), the remaining two run `x00` upward within it in a
//! deliberate order. A code's number is part of the language's surface: it is what a
//! reader searches for and what `quilon explain` answers, so an existing number is never
//! reassigned; a new code takes the next free number in its family.
//!
//! The runtime's codes are mirrored as plain constants in `quilon-rt` (`report::codes`),
//! since a compiled program reports without the compiler; a test here pins the two.
//!
//! The explanations live in `docs/tooling/errors.md`, one section per code, embedded at
//! compile time — [`explain`] slices the section, so the prose has one home.

macro_rules! codes {
    ($($name:ident = $number:literal => $title:literal,)*) => {
        /// One diagnostic kind. The discriminant is the code's number.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        #[repr(u16)]
        pub enum Code {
            $($name = $number,)*
        }

        /// Every code, in numeric order.
        pub const ALL: &[Code] = &[$(Code::$name,)*];

        impl Code {
            /// The one-line title the registry gives this code.
            pub fn title(self) -> &'static str {
                match self {
                    $(Code::$name => $title,)*
                }
            }
        }
    };
}

codes! {
    // Family 0 — lexer (input included: a source file is read before it is lexed)
    SourceNotReadable = 0 => "unreadable source file",
    NotAQuilonSource = 1 => "source file with an extension other than `.qn`",
    InvalidToken = 2 => "invalid token",
    UnterminatedString = 3 => "unterminated string literal",
    BidiControl = 4 => "misplaced bidirectional control character",

    // Family 1 — parser
    UnexpectedToken = 100 => "unexpected token",
    NestingTooDeep = 101 => "expression nesting too deep",
    TooManyParameters = 102 => "too many parameters",
    EmptyMatch = 103 => "match with no arms",
    InterpolationHole = 104 => "interpolation hole with more than one expression",
    ImportPathInterpolated = 105 => "import path with interpolation",
    NotAnImportedModule = 106 => "qualified name through a missing import",
    AmbiguousTypeDeclaration = 107 => "ambiguous `{ }` type declaration",
    OperatorMemberMutable = 108 => "operator member declared with `:=`",
    VariantNotCapitalized = 109 => "lowercase sum-type variant",
    SumTypeHasFields = 110 => "sum type with fields or a mutating method",
    BodyNotABlock = 111 => "bare expression as a function body",
    ExportMarkerAsBlockClosers = 112 => "`>>` where two block closers were meant",

    // Family 2 — module resolution and linking
    AtDeclarationOutsideCorelib = 200 => "`@` primitive declared outside the corelib",
    UnknownModule = 201 => "missing module",
    NotExported = 202 => "private member reached through its module",
    NameClaimedByImport = 203 => "name claimed by an import",
    ImportCycle = 204 => "import cycle",
    ModuleNameCollision = 205 => "two modules with one name",
    ModuleIsNotAValue = 206 => "module binding used as a value",
    AmbiguousModulePrefix = 207 => "ambiguous module prefix",
    NoTestHarness = 208 => "test blocks without `core.test`",

    // Family 3 — type checker
    UndefinedVariable = 300 => "undefined name",
    TypeMismatch = 301 => "type mismatch",
    NotAFunction = 302 => "call on a data value",
    WrongNumberOfArguments = 303 => "wrong number of arguments",
    ImmutableAssignment = 304 => "assignment to an immutable binding",
    ImmutableFieldWrite = 305 => "field write through an immutable binding",
    MutableAliasOfImmutable = 306 => "`:=` binding aliasing an immutable value",
    ImmutableAliasOfMutable = 307 => "`=` binding aliasing a mutable value",
    MutatingMethodOnImmutable = 308 => "mutating method on an immutable receiver",
    MutatingMethodDeclaredImmutable = 309 => "mutating method declared with `=`",
    DuplicateDefinition = 310 => "duplicate definition",
    NoMatchingOverload = 311 => "no matching overload",
    AmbiguousOverload = 312 => "ambiguous overload",
    OverloadMissingAnnotation = 313 => "overload member with an unannotated parameter",
    UnannotatedParameter = 314 => "parameter without a type",
    SignatureArity = 315 => "parameter count differs from the function type",
    UninferableLambdaParameter = 316 => "lambda parameter with an open type",
    RecursiveFunctionNeedsReturnType = 317 => "recursive function without a return type",
    SiteIsImmutable = 318 => "write to a `Site` field",
    MisplacedSiteParameter = 319 => "misplaced `Site` parameter",
    OverloadCallBeforeDefinition = 320 => "call before the definition",
    UnannotatedOverloadMember = 321 => "overload member without a return type",
    ComparisonOverloadNotBool = 322 => "comparison operator returning a type other than `Bool`",
    RefutableConstructorArg = 323 => "nested pattern inside a constructor pattern",
    NonExhaustiveMatch = 324 => "non-exhaustive match",
    UnknownConstructor = 325 => "unknown variant",
    ConstructorPatternOnNonSum = 326 => "constructor pattern on a non-sum value",
    InvalidEntryPointSignature = 327 => "unsupported `^` signature",
    InvalidBuiltinArgument = 328 => "invalid argument to a built-in",
    ComputedGlobalBinding = 329 => "top-level binding that has to be computed",
    OperatorMustBeMember = 330 => "operator defined at the top level",
    OperatorMemberArity = 331 => "operator member with the wrong parameter count",
    AssertionNeedsMatcher = 332 => "assertion without a matcher",
    ExpectOutsideTest = 333 => "`expect` outside a test case",
    MatcherArity = 334 => "matcher with the wrong argument count",
    MatcherTypeUnsupported = 335 => "matcher on a type outside its reach",
    UnknownMember = 336 => "unknown member",
    MethodCalledAsFunction = 337 => "method called as a function",
    NotRenderable = 338 => "value with no rendering",
    NoEntryPoint = 339 => "no `^` entry point",

    // Family 4 — codegen and build
    CodegenFailed = 400 => "code generation failed",
    BuildFailed = 401 => "native build failed",

    // Family 5 — runtime
    AssertionFailed = 500 => "assertion failed",
    IndexOutOfBounds = 501 => "index out of bounds",
    RangeEndpointNotWhole = 502 => "fractional or unrepresentable range endpoint",
    MatchFailed = 503 => "no arm matched",
    AllocationFailed = 504 => "allocation failed",
    ReadFailed = 505 => "reading stdin failed",
}

impl Code {
    /// The code's number — what `QN012` carries.
    pub fn number(self) -> u16 {
        self as u16
    }

    /// The code named `text` (`QN012`, or bare `12`), if the registry has it.
    pub fn parse(text: &str) -> Option<Code> {
        let digits = text
            .strip_prefix("QN")
            .or_else(|| text.strip_prefix("qn"))
            .unwrap_or(text);
        let number: u16 = digits.parse().ok()?;
        ALL.iter().copied().find(|code| code.number() == number)
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "QN{:03}", self.number())
    }
}

/// The reference every explanation is read from, embedded at compile time.
const REFERENCE: &str = include_str!("../../docs/tooling/errors.md");

/// The heading that opens `code`'s section in the reference.
fn heading(code: Code) -> String {
    format!("### {code}")
}

/// `code`'s section of the reference — its heading through the line before the next
/// heading — or `None` when the reference has no section for it.
pub fn explain(code: Code) -> Option<&'static str> {
    let heading = heading(code);
    let start = REFERENCE
        .lines()
        .scan(0, |offset, line| {
            let at = *offset;
            *offset += line.len() + 1;
            Some((at, line))
        })
        .find(|(_, line)| line.starts_with(&heading))
        .map(|(at, _)| at)?;
    let body = &REFERENCE[start..];
    let end = body
        .match_indices("\n## ")
        .chain(body.match_indices("\n### "))
        .map(|(at, _)| at + 1)
        .min()
        .unwrap_or(body.len());
    Some(body[..end].trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code carries a number no other code repeats — the registry is the single
    /// place that assigns them, and a collision would be two failures answering to one
    /// `quilon explain`.
    #[test]
    fn every_code_has_a_unique_number() {
        let mut numbers: Vec<u16> = ALL.iter().map(|code| code.number()).collect();
        let before = numbers.len();
        numbers.sort_unstable();
        numbers.dedup();
        assert_eq!(
            numbers.len(),
            before,
            "a code number repeats in the registry"
        );
    }

    #[test]
    fn a_code_renders_and_parses_as_qn_and_three_digits() {
        assert_eq!(Code::InvalidToken.to_string(), "QN002");
        assert_eq!(Code::parse("QN002"), Some(Code::InvalidToken));
        assert_eq!(Code::parse("qn2"), Some(Code::InvalidToken));
        assert_eq!(Code::parse("QN999"), None);
        assert_eq!(Code::parse("nope"), None);
    }

    /// Every registered code has its section in the reference, and every section in the
    /// reference names a registered code — the docs and the registry describe one set.
    #[test]
    fn every_code_is_explained_in_the_reference() {
        for code in ALL {
            let section = explain(*code).unwrap_or_else(|| panic!("no section for {code}"));
            assert!(
                section.starts_with(&heading(*code)),
                "{code}'s section starts with its heading: {section}"
            );
            assert!(
                section.contains(code.title()),
                "{code}'s heading carries the registry title {:?}: {section}",
                code.title()
            );
            assert!(
                section.contains("```"),
                "{code}'s section shows a minimal example: {section}"
            );
        }
        let documented = REFERENCE
            .lines()
            .filter_map(|line| line.strip_prefix("### "))
            .map(|rest| rest.split_whitespace().next().unwrap_or_default())
            .filter(|word| word.starts_with("QN"))
            .count();
        assert_eq!(documented, ALL.len(), "sections and codes are one set");
    }

    /// The `## The codes` summary table lists exactly the registered codes, in ascending
    /// order, each with the registry's own title — the table is read by eye far more often
    /// than any single section, so it drifting from the registry is the likeliest way this
    /// reference goes stale.
    #[test]
    fn the_summary_table_matches_the_registry() {
        let heading_line = REFERENCE
            .lines()
            .position(|line| line.trim() == "## The codes")
            .expect("the reference has a `## The codes` heading");
        let rows: Vec<(u16, &str)> = REFERENCE
            .lines()
            .skip(heading_line + 1)
            .take_while(|line| !line.trim_start().starts_with("## "))
            .filter_map(|line| {
                let line = line.trim();
                let cells = line.strip_prefix("| QN")?;
                let (number, rest) = cells.split_once(" | ")?;
                let title = rest.strip_suffix(" |")?;
                Some((number.parse().ok()?, title))
            })
            .collect();

        let registry: Vec<(u16, &str)> = ALL
            .iter()
            .map(|code| (code.number(), code.title()))
            .collect();
        assert_eq!(
            rows, registry,
            "the summary table's rows (number, title) must match the registry, in order"
        );
    }

    /// The runtime's copies of its own codes match the registry.
    #[test]
    fn the_runtime_mirrors_its_codes() {
        use quilon_rt::report::codes;
        assert_eq!(codes::ASSERTION_FAILED, Code::AssertionFailed.number());
        assert_eq!(codes::INDEX_OUT_OF_BOUNDS, Code::IndexOutOfBounds.number());
        assert_eq!(
            codes::RANGE_ENDPOINT_NOT_WHOLE,
            Code::RangeEndpointNotWhole.number()
        );
        assert_eq!(codes::MATCH_FAILED, Code::MatchFailed.number());
        assert_eq!(codes::ALLOCATION_FAILED, Code::AllocationFailed.number());
        assert_eq!(codes::READ_FAILED, Code::ReadFailed.number());
    }
}
