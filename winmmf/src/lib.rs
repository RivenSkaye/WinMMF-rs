//! # Memory-Mapped Files, Rust-style
//!
//! This crate contains everything you need to work with Memory-Mapped files. Or you can just roll your own and build
//! upon the [`Mmf`] trait defined here. This module exports some utilities and ease of use items and you're entirely
//! free to use or not use them. By default, the implementations and namespaces are enabled. If you do not wish to do
//! so, look at the implementation for [`MemoryMappedFile`] and check the `use` statements to see what you need to do to
//! get things working.
//!
//! The internal implementation is built entirely around using [`fixedstr::zstr`] to keep references to strings alive
//! because for some reason everything goes to hell if you don't. [`microseh`] is just as much a core component here, as
//! it's a requirement to get the OS to play nice in the event of something going wrong and a structured exception being
//! thrown. This **does** mean that you, the consumer of this library, must ensure a clean exit and teardown upon
//! failure. No, a [`panic!`] does not suffice, ensure things get dropped and that the OS doesn't unwind your ass.
//!
//! Most of the interesting and relevant bits are located [in the `mmf` module][mmf].

pub mod err;
pub mod mmf;
pub mod states;

pub use err::*;
pub use mmf::*;
pub use states::*;

#[cfg(test)]
mod unit_tests;
