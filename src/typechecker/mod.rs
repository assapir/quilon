// Type checker module for Quilon

pub mod checker;

pub use checker::{MatcherHoverTable, TypeChecker, TypeTable};
// Shared with codegen so array `+` classifies concat-vs-append/prepend with the SAME
// element-type equality the checker used (avoids the two sites drifting apart).
pub(crate) use checker::types_match;
