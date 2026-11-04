//! Integration with wxDragon, enabled by the `wx` feature.
//!
//! wxWidgets has its own translations for stock strings, such as the labels on standard buttons.
//! This module keeps those translations and the catalog of the app on the same language, and helps
//! to fill a language selector.

use wxdragon::{
	translations::{Translations, TranslationsLoader},
	widgets::choice::Choice,
};

use crate::{LanguageInfo, LanguageManager, best_available_language};

/// Keeps the language of a domain and the language of wxWidgets the same.
///
/// Create one for each app and call [`Self::initialize`] before you create any UI. The type has no
/// global instance of its own, so an app that needs one owns it, for example in a `OnceLock`.
pub struct WxTranslationManager {
	domain: String,
	current_language: String,
	available_languages: Vec<LanguageInfo>,
	initialized: bool,
}

impl WxTranslationManager {
	/// Creates a manager for `domain`.
	#[must_use]
	pub fn new(domain: &str) -> Self {
		Self {
			domain: domain.to_string(),
			current_language: "en".to_string(),
			available_languages: vec![LanguageInfo { code: "en".to_string(), name: "English".to_string() }],
			initialized: false,
		}
	}

	/// Activates the language that best fits the system, or English if none fits, in both the
	/// domain and wxWidgets. Later calls do nothing.
	///
	/// `wxstd_loader` is normally the `WxStdCatalogLoader` type that
	/// [`embed_wx_translations!`](crate::embed_wx_translations) generates.
	///
	/// # Panics
	///
	/// Panics if a thread panicked while it held the lock on the global translation state.
	pub fn initialize(&mut self, wxstd_loader: impl TranslationsLoader + 'static) {
		if self.initialized {
			return;
		}
		let translations = Translations::new();
		// The loader must be in place before `add_std_catalog`, or wxWidgets doesn't consult it.
		translations.set_loader(wxstd_loader);
		self.available_languages = LanguageManager::new(&self.domain).available();
		self.current_language = best_available_language(&LanguageManager::system_language(), &self.available_languages)
			.unwrap_or_else(|| "en".to_string());
		if self.current_language != "en" {
			translations.set_language_str(&self.current_language);
		}
		translations.add_std_catalog();
		Translations::set_global(translations);
		crate::set_default_domain(&self.domain);
		crate::set_locale(&self.current_language);
		self.initialized = true;
	}

	/// Switches both the domain and wxWidgets to `language_code`, and returns whether it switched.
	///
	/// Returns `false` if the manager isn't initialized or the language isn't available.
	///
	/// # Panics
	///
	/// Panics if a thread panicked while it held the lock on the global translation state.
	pub fn set_language(&mut self, language_code: &str, wxstd_loader: impl TranslationsLoader + 'static) -> bool {
		if !self.initialized || !self.is_language_available(language_code) {
			return false;
		}
		self.current_language = language_code.to_string();
		// A new `Translations` needs its own loader before `add_std_catalog` picks the catalog.
		let translations = Translations::new();
		translations.set_loader(wxstd_loader);
		translations.set_language_str(language_code);
		translations.add_std_catalog();
		Translations::set_global(translations);
		crate::set_locale(language_code);
		true
	}

	/// Returns the code of the active language.
	#[must_use]
	pub fn current_language(&self) -> String {
		self.current_language.clone()
	}

	/// Returns the languages that this manager can switch to.
	#[must_use]
	pub fn available_languages(&self) -> Vec<LanguageInfo> {
		self.available_languages.clone()
	}

	/// Returns whether this manager can switch to `language_code`.
	#[must_use]
	pub fn is_language_available(&self, language_code: &str) -> bool {
		self.available_languages.iter().any(|lang| lang.code == language_code)
	}
}

/// Adds the name of each language in `languages` to `combo`, and returns their codes in the same
/// order, for use with [`resolve_language_choice`].
#[must_use]
pub fn populate_language_choice(combo: &Choice, languages: &[LanguageInfo]) -> Vec<String> {
	let mut codes = Vec::with_capacity(languages.len());
	for lang in languages {
		combo.append(&lang.name);
		codes.push(lang.code.clone());
	}
	codes
}

/// Returns the code of the language selected in `combo`, or `None` if nothing is selected.
///
/// `codes` is the list that [`populate_language_choice`] returned.
#[must_use]
pub fn resolve_language_choice(combo: &Choice, codes: &[String]) -> Option<String> {
	combo.get_selection().and_then(|index| usize::try_from(index).ok()).and_then(|index| codes.get(index).cloned())
}

#[cfg(test)]
mod tests {
	use std::borrow::Cow;

	use super::*;

	struct NoopLoader;

	impl TranslationsLoader for NoopLoader {
		fn load_catalog(&self, _domain: &str, _lang: &str) -> Option<Cow<'_, [u8]>> {
			None
		}

		fn available_translations(&self, _domain: &str) -> Vec<String> {
			Vec::new()
		}
	}

	#[test]
	fn new_manager_has_english_available_by_default() {
		let manager = WxTranslationManager::new("test_domain");
		assert_eq!(manager.current_language(), "en");
		assert!(manager.is_language_available("en"));
		assert!(!manager.is_language_available("zz"));
	}

	#[test]
	fn set_language_fails_when_not_initialized() {
		let mut manager = WxTranslationManager::new("test_domain");
		assert!(!manager.set_language("en", NoopLoader));
		assert_eq!(manager.current_language(), "en");
	}
}
