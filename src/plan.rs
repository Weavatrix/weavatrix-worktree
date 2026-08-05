//! Executor-only projection from a validated refactor contract to path transitions.

mod compile;

pub(crate) use compile::{PathTransition, PlannedInput, PlannedOutput, compile_plan};
