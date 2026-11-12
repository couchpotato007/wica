//! Where tool information comes from. Each source is independent; the index
//! merges them by command name.
//!
//! Package manager descriptions (dpkg, pacman, rpm, nix, brew) are the next
//! source to add here.

pub mod man;
pub mod owners;
pub mod path;
pub mod tags;
pub mod tldr;
