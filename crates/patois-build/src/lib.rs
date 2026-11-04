//! Gettext tooling for apps that use [patois](https://docs.rs/patois). Call it from a build script
//! or an `xtask`.
//!
//! - [`gen_pot`] and [`gen_pot_from_dirs`] extract strings from Rust source into a `.pot` template.
//! - [`extend_pot_from_source_dirs`] and [`prune_pot_from_source_dirs`] keep the strings of Swift
//!   and Kotlin source in the same template.
//! - [`compile_translations`] compiles `.po` catalogs to the `.mo` files that an app embeds.
//! - [`gen_ios_strings`] and [`gen_android_strings`] generate translation assets for mobile apps.
//! - [`check`] and [`lint`] find translation mistakes in catalogs and source code.
//! - [`credits`] reads the names of translators from the catalogs.
//! - [`po`] edits a `.po` file and leaves the lines it doesn't change as they were.

pub mod check;
pub mod credits;
mod entries;
mod extract;
pub mod lint;
mod mo;
mod mobile;
pub mod po;
mod pot;
mod sanitize_rust;

pub use extract::{extend_pot_from_source_dirs, prune_pot_from_source_dirs};
pub use mo::compile_translations;
pub use mobile::{gen_android_strings, gen_ios_strings};
pub use pot::{gen_pot, gen_pot_from_dirs};
