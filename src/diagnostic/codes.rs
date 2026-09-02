//! The error-code registry: every diagnostic the compiler or a compiled program can raise,
//! numbered `Q000` upward in pipeline order — input, lexer, parser, imports, checker,
//! codegen and build, runtime. A code's number is part of the language's surface: it is
//! what a reader searches for and what `quilon explain` answers, so an existing number is
//! never reassigned; a new code takes the next number in its group's range, or the next
//! free number at the end.
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
    // Input
    SourceNotReadable = 0 => "unreadable source file",
    NotAQuilonSource = 1 => "source file with an extension other than `.qn`",

    // Lexer
    InvalidToken = 2 => "invalid token",
    UnterminatedString = 3 => "unterminated string literal",
    BidiControl = 4 => "misplaced bidirectional control character",

    // Parser
    UnexpectedToken = 5 => "unexpected token",
    NestingTooDeep = 6 => "expression nesting too deep",
    TooManyParameters = 7 => "too many parameters",
    EmptyMatch = 8 => "match with no arms",
    InterpolationHole = 9 => "interpolation hole with more than one expression",
    ImportPathInterpolated = 10 => "import path with interpolation",
    NotAnImportedModule = 11 => "qualified name through a missing import",
    AmbiguousTypeDeclaration = 12 => "ambiguous `{ }` type declaration",
    OperatorMemberMutable = 13 => "operator member declared with `:=`",
    VariantNotCapitalized = 14 => "lowercase sum-type variant",
    SumTypeHasFields = 15 => "sum type with fields or a mutating method",
    BodyNotABlock = 16 => "bare expression as a function body",
    ExportMarkerAsBlockClosers = 17 => "`>>` where two block closers were meant",

    // Imports
    AtDeclarationOutsideCorelib = 18 => "`@` primitive declared outside the corelib",
    UnknownModule = 19 => "missing module",
    NotExported = 20 => "private member reached through its module",
    NameClaimedByImport = 21 => "name claimed by an import",
    ImportCycle = 22 => "import cycle",
    ModuleNameCollision = 23 => "two modules with one name",
    ModuleIsNotAValue = 24 => "module binding used as a value",
    AmbiguousModulePrefix = 25 => "ambiguous module prefix",
    NoTestHarness = 26 => "test blocks without `core.test`",

    // Checker
    UndefinedVariable = 27 => "undefined name",
    TypeMismatch = 28 => "type mismatch",
    NotAFunction = 29 => "call on a data value",
    WrongNumberOfArguments = 30 => "wrong number of arguments",
    ImmutableAssignment = 31 => "assignment to an immutable binding",
    ImmutableFieldWrite = 32 => "field write through an immutable binding",
    MutableAliasOfImmutable = 33 => "`:=` binding aliasing an immutable value",
    ImmutableAliasOfMutable = 34 => "`=` binding aliasing a mutable value",
    MutatingMethodOnImmutable = 35 => "mutating method on an immutable receiver",
    MutatingMethodDeclaredImmutable = 36 => "mutating method declared with `=`",
    DuplicateDefinition = 37 => "duplicate definition",
    NoMatchingOverload = 38 => "no matching overload",
    AmbiguousOverload = 39 => "ambiguous overload",
    OverloadMissingAnnotation = 40 => "overload member with an unannotated parameter",
    UnannotatedParameter = 41 => "parameter without a type",
    SignatureArity = 42 => "parameter count differs from the function type",
    UninferableLambdaParameter = 43 => "lambda parameter with an open type",
    RecursiveFunctionNeedsReturnType = 44 => "recursive function without a return type",
    SiteIsImmutable = 45 => "write to a `Site` field",
    MisplacedSiteParameter = 46 => "misplaced `Site` parameter",
    OverloadCallBeforeDefinition = 47 => "call before the definition",
    UnannotatedOverloadMember = 48 => "overload member without a return type",
    ComparisonOverloadNotBool = 49 => "comparison operator returning a type other than `Bool`",
    RefutableConstructorArg = 50 => "nested pattern inside a constructor pattern",
    NonExhaustiveMatch = 51 => "non-exhaustive match",
    UnknownConstructor = 52 => "unknown variant",
    ConstructorPatternOnNonSum = 53 => "constructor pattern on a non-sum value",
    InvalidEntryPointSignature = 54 => "unsupported `^` signature",
    InvalidBuiltinArgument = 55 => "invalid argument to a built-in",
    ComputedGlobalBinding = 56 => "top-level binding that has to be computed",
    OperatorMustBeMember = 57 => "operator defined at the top level",
    OperatorMemberArity = 58 => "operator member with the wrong parameter count",
    AssertionNeedsMatcher = 59 => "assertion without a matcher",
    ExpectOutsideTest = 60 => "`expect` outside a test case",
    MatcherArity = 61 => "matcher with the wrong argument count",
    MatcherTypeUnsupported = 62 => "matcher on a type outside its reach",
    UnknownMember = 63 => "unknown member",
    MethodCalledAsFunction = 64 => "method called as a function",
    NotRenderable = 65 => "value with no rendering",
    NoEntryPoint = 66 => "no `^` entry point",

    // Codegen and build
    CodegenFailed = 67 => "code generation failed",
    BuildFailed = 68 => "native build failed",

    // Runtime
    AssertionFailed = 69 => "assertion failed",
    IndexOutOfBounds = 70 => "index out of bounds",
    RangeEndpointNotWhole = 71 => "fractional or unrepresentable range endpoint",
    MatchFailed = 72 => "no arm matched",
    AllocationFailed = 73 => "allocation failed",
    ReadFailed = 74 => "reading stdin failed",
}

impl Code {
    /// The code's number — what `Q012` carries.
    pub fn number(self) -> u16 {
        self as u16
    }

    /// The code named `text` (`Q012`, or bare `12`), if the registry has it.
    pub fn parse(text: &str) -> Option<Code> {
        let digits = text
            .strip_prefix('Q')
            .or_else(|| text.strip_prefix('q'))
            .unwrap_or(text);
        let number: u16 = digits.parse().ok()?;
        ALL.iter().copied().find(|code| code.number() == number)
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Q{:03}", self.number())
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

    /// Numbers are sequential from zero with no gaps — the registry is the single place
    /// that assigns them, and a gap would be a code that was silently dropped.
    #[test]
    fn codes_are_sequential_from_zero() {
        for (index, code) in ALL.iter().enumerate() {
            assert_eq!(code.number() as usize, index, "{code:?} is out of sequence");
        }
    }

    #[test]
    fn a_code_renders_and_parses_as_q_and_three_digits() {
        assert_eq!(Code::InvalidToken.to_string(), "Q002");
        assert_eq!(Code::parse("Q002"), Some(Code::InvalidToken));
        assert_eq!(Code::parse("q2"), Some(Code::InvalidToken));
        assert_eq!(Code::parse("Q999"), None);
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
            .filter(|word| word.starts_with('Q'))
            .count();
        assert_eq!(documented, ALL.len(), "sections and codes are one set");
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
