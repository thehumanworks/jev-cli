//! Command-line client for `TypeSafe`'s Jev model.
//!
//! The binary is a thin wrapper around [`run`]. Tests and embeddings call [`run`] with in-memory
//! pipes and, when they should not open a socket, a `typesafe_jev::Transport`.

mod call;
mod cli;
mod decide;
mod exit;
mod preset;
mod questions;
mod render;
mod run;
mod state;

#[cfg(test)]
mod e2e;

pub use run::{Io, run};
