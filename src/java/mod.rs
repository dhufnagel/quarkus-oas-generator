//! Java sources, parsed by jtk and reduced to the declarations the index needs.

pub mod ast;
mod convert;
mod literal;

pub use convert::parse;
