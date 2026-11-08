//! Gettext translations for Rust desktop apps.
//!
//! An app embeds its compiled catalogs in the binary and calls [`init_auto`] at startup. Code in
//! the app, and in any library the app depends on, then calls [`t`] and [`nt`] to get translated
//! strings.
//!
//! ```rust,no_run
//! use patois::{nt, t};
//!
//! patois::embed_domain!();
//!
//! fn main() {
//!     patois::init_auto(env!("CARGO_PKG_NAME"));
//!     println!("{}", t("Checking for updates..."));
//!     println!("{}", nt("%d file", "%d files", 3).replacen("%d", "3", 1));
//! }
//! ```
//!
//! A library calls [`t`] and [`nt`] and needs no setup of its own: the strings resolve against the
//! catalog of the app that uses the library.

mod catalog;
mod lang;
mod plural;
mod registry;
#[cfg(feature = "wx")]
pub mod wx;

use std::path::Path;

#[doc(hidden)]
pub use inventory;
pub use lang::{LanguageInfo, LanguageManager, best_available_language, language_name};
pub use patois_macros::embed_domain;
#[cfg(feature = "wx")]
pub use patois_macros::embed_wx_translations;

/// The catalogs that [`embed_domain!`] registers for one crate.
#[doc(hidden)]
pub struct EmbeddedDomain {
	pub name: &'static str,
	/// The `.mo` catalog of each locale, as `msgfmt` wrote it.
	pub files: &'static [(&'static str, &'static [u8])],
}

inventory::collect!(EmbeddedDomain);

/// Sets up translations for an app that loads its catalogs from disk.
///
/// Registers the locale directory of `domain`, makes `domain` the active domain, and activates
/// `locale_id`. The catalogs must be at `<locale_dir>/<locale>/LC_MESSAGES/<domain>.mo`.
///
/// [`LanguageManager`] and [`available_locales`] list embedded catalogs only, so they don't see
/// catalogs loaded this way.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
pub fn init(domain: &str, locale_dir: impl AsRef<Path>, locale_id: &str) {
	let mut reg = registry::global().lock().unwrap();
	reg.register_path(domain, locale_dir.as_ref().to_path_buf());
	reg.set_default_domain(domain);
	reg.set_locale(locale_id);
}

/// Sets up translations for an app that embeds its catalogs with [`embed_domain!`].
///
/// Makes `domain` the active domain and activates the catalog that best fits the system language,
/// as [`best_available_language`] chooses it. If no catalog fits, strings stay in English.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
pub fn init_auto(domain: &str) {
	let languages = LanguageManager::new(domain).available();
	let locale = best_available_language(&system_locale(), &languages).unwrap_or_else(|| "en".to_string());
	let mut reg = registry::global().lock().unwrap();
	reg.set_default_domain(domain);
	reg.set_locale(&locale);
}

/// Returns the translation of `s` in the active locale.
///
/// Returns `s` unchanged if no domain is active or the catalog has no translation.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
#[must_use]
pub fn t(s: &str) -> String {
	registry::global().lock().unwrap().translate(s)
}

/// Returns the form of a plural string that fits `n` in the active locale.
///
/// The rule comes from the `Plural-Forms` header of the catalog. If no domain is active or the
/// catalog has no translation, returns `singular` when `n` is 1 and `plural` otherwise.
///
/// The result still contains its placeholder, so replace that with the number yourself.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
#[must_use]
pub fn nt(singular: &str, plural: &str, n: u64) -> String {
	registry::global().lock().unwrap().translate_plural(singular, plural, n)
}

/// Changes the active locale.
///
/// Later calls to [`t`] and [`nt`] use the new locale. Case and the separator don't matter, so
/// `pt-BR`, `pt_br`, and `pt_BR` all select a catalog named `pt_br`. If there is no catalog for the
/// region, the catalog for the bare language is used.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
pub fn set_locale(locale_id: &str) {
	registry::global().lock().unwrap().set_locale(locale_id);
}

/// Returns the active locale in normalized form, such as `pt_BR`.
///
/// Returns an empty string before a locale is set.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
#[must_use]
pub fn locale() -> String {
	registry::global().lock().unwrap().locale().to_string()
}

/// Makes `domain` the active domain without registering a locale directory.
///
/// Use this function when [`embed_domain!`] embeds the catalogs of `domain` and you choose the
/// locale yourself.
///
/// # Panics
///
/// Panics if a thread panicked while it held the lock on the global translation state.
pub fn set_default_domain(domain: &str) {
	registry::global().lock().unwrap().set_default_domain(domain);
}

/// Returns the locale codes of the catalogs embedded for `domain`.
///
/// The codes are the names of the directories under `locale/` at compile time, such as `de` and
/// `pt_br`.
#[must_use]
pub fn available_locales(domain: &str) -> Vec<&'static str> {
	inventory::iter::<EmbeddedDomain>
		.into_iter()
		.filter(|ed| ed.name == domain)
		.flat_map(|ed| ed.files.iter().map(|(locale, _)| *locale))
		.collect()
}

/// Returns the preferred locale of the operating system in normalized form, such as `en_US`.
///
/// Returns `en` if the system locale can't be determined.
#[must_use]
pub fn system_locale() -> String {
	sys_locale::get_locale().map_or_else(|| "en".to_string(), |l| registry::normalize_locale(&l))
}
