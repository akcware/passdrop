//! passdrop core: spec parsing, value warnings, destination guards, sinks and the
//! result contract. No GUI code lives here (DESIGN.md §4).
//!
//! Rule for every module: error messages, `Debug` output and results may carry key
//! names, paths and line numbers, never a value.

pub mod generate;
pub mod git;
pub mod guards;
pub mod registry;
pub mod result;
pub mod run;
pub mod sinks;
pub mod spec;
pub mod validate;

pub use secrecy::{ExposeSecret, SecretString};
