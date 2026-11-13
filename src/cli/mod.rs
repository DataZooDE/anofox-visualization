//! The command-line programs as library functions (#1156): the binaries in
//! `src/bin/` only call them, and a distribution crate runs them with its
//! modules registered first ([`crate::module::register`]).

pub mod dashboard;
