//! Internal modules shared by the `ite` binary and its test/support programs.
//!
//! This crate is an implementation detail of the end-user application, not a
//! supported library API.

pub mod app;
pub mod cli;
pub mod config;
pub mod fstree;
pub mod json_tree;
pub mod jump;
pub mod keybindings;
pub mod keys;
pub mod profile;
pub mod runner;
pub mod tree;
pub mod ui;
