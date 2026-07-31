//! Internal implementation crate shared by the `ite` binary, integration
//! tests, and support programs. This is not a supported public library API.
//!
//! Source adapters (`fstree`, `json_tree`) produce the neutral `tree` model.
//! `app` coordinates that model with `keys`, `config`, `jump`, and
//! `keybindings`; `ui` renders the result. Process and terminal I/O stay at the
//! binary boundary, with `runner` and `opener` providing the subprocess edges
//! for shell bindings and the platform's default file opener.

pub mod app;
pub mod cli;
pub mod config;
pub mod fstree;
pub mod json_tree;
pub mod jump;
pub mod keybindings;
pub mod keys;
pub mod opener;
pub mod profile;
pub mod runner;
pub mod tree;
pub mod ui;
