use crate::{available_locales, locale, registry::normalize_locale, set_locale};

/// A language that an app offers.
#[derive(Debug, Clone)]
pub struct LanguageInfo {
	/// The name of the catalog's directory under `locale/`, such as `de` or `pt_br`.
	pub code: String,
	/// The language's name in that language, for a language selector, such as `Deutsch`.
	pub name: String,
}

/// Lists the languages of a domain and switches between them.
///
/// Sees embedded catalogs only, because those are the only ones known without scanning a directory
/// that the app chose.
///
/// # Examples
///
/// ```rust,no_run
/// patois::embed_domain!();
///
/// fn main() {
///     patois::init_auto(env!("CARGO_PKG_NAME"));
///     let languages = patois::LanguageManager::new(env!("CARGO_PKG_NAME"));
///     for language in languages.available() {
///         println!("{}: {}", language.code, language.name);
///     }
///     if !languages.set("de") {
///         eprintln!("This app has no German catalog.");
///     }
/// }
/// ```
pub struct LanguageManager {
	domain: String,
}

impl LanguageManager {
	/// Creates a manager for `domain`.
	#[must_use]
	pub fn new(domain: &str) -> Self {
		Self { domain: domain.to_string() }
	}

	/// Returns the available languages: English first, then the others sorted by native name.
	#[must_use]
	pub fn available(&self) -> Vec<LanguageInfo> {
		let mut langs = vec![LanguageInfo { code: "en".to_string(), name: "English".to_string() }];
		let mut others: Vec<LanguageInfo> = available_locales(&self.domain)
			.into_iter()
			.filter(|&code| code != "en")
			.map(|code| LanguageInfo { code: code.to_string(), name: language_name(code).to_string() })
			.collect();
		others.sort_by(|a, b| a.name.cmp(&b.name));
		langs.extend(others);
		langs
	}

	/// Returns the active locale, spelled like the [`LanguageInfo::code`] of its catalog when
	/// there is one, so that the result can be compared with [`Self::available`].
	///
	/// # Panics
	///
	/// Panics if a thread panicked while it held the lock on the global translation state.
	#[must_use]
	pub fn current(&self) -> String {
		let active = locale();
		available_locales(&self.domain)
			.into_iter()
			.find(|code| normalize_locale(code) == active)
			.map_or(active, str::to_string)
	}

	/// Switches to the language `code`, and returns whether the language is available.
	///
	/// English is always available. Case and the separator in `code` don't matter.
	///
	/// # Panics
	///
	/// Panics if a thread panicked while it held the lock on the global translation state.
	#[must_use]
	pub fn set(&self, code: &str) -> bool {
		let wanted = normalize_locale(code);
		if wanted == "en" || available_locales(&self.domain).iter().any(|c| normalize_locale(c) == wanted) {
			set_locale(code);
			true
		} else {
			false
		}
	}

	/// Returns the preferred locale of the operating system, or `en` if it can't be determined.
	#[must_use]
	pub fn system_language() -> String {
		crate::system_locale()
	}
}

/// Returns the code of the language in `available` that best fits `system_code`, or `None` if no
/// language fits.
///
/// A system locale often names more or less than a catalog does. The function tries these matches
/// in order, ignoring case and the separator:
///
/// 1. The same locale.
/// 2. The most specific catalog whose parts all appear in the system locale, so `de_DE` finds `de`,
///    and `zh-Hans-CN` finds `zh_CN`.
/// 3. Any catalog for the language, so a reader in Taiwan gets Simplified Chinese instead of
///    English.
#[must_use]
pub fn best_available_language(system_code: &str, available: &[LanguageInfo]) -> Option<String> {
	let wanted = normalize_locale(system_code);
	let wanted_parts: Vec<&str> = wanted.split('_').collect();
	let catalogs: Vec<(&LanguageInfo, String)> = available.iter().map(|l| (l, normalize_locale(&l.code))).collect();
	let same_language: Vec<(&LanguageInfo, Vec<&str>)> = catalogs
		.iter()
		.map(|(l, code)| (*l, code.split('_').collect::<Vec<_>>()))
		.filter(|(_, parts)| parts[0] == wanted_parts[0])
		.collect();
	catalogs
		.iter()
		.find(|(_, code)| *code == wanted)
		.map(|(l, _)| *l)
		.or_else(|| {
			same_language
				.iter()
				.filter(|(_, parts)| parts.iter().all(|part| wanted_parts.contains(part)))
				.max_by_key(|(_, parts)| parts.len())
				.map(|(l, _)| *l)
		})
		.or_else(|| same_language.first().map(|(l, _)| *l))
		.map(|l| l.code.clone())
}

/// Returns the name of the language `code` in that language, or `code` if the language is unknown.
///
/// Case doesn't matter, but the separator must be an underscore.
#[must_use]
pub fn language_name(code: &str) -> &str {
	match code.to_lowercase().as_str() {
		"af" => "Afrikaans",
		"ar" => "العربية",
		"bg" => "Български",
		"bs" => "Bosanski",
		"ca" => "Català",
		"cs" => "Čeština",
		"cy" => "Cymraeg",
		"da" => "Dansk",
		"de" | "de_at" | "de_ch" => "Deutsch",
		"el" => "Ελληνικά",
		"en" | "en_us" | "en_gb" => "English",
		"eo" => "Esperanto",
		"es" | "es_419" => "Español",
		"et" => "Eesti",
		"eu" => "Euskara",
		"fa" => "فارسی",
		"fi" => "Suomi",
		"fr" | "fr_be" | "fr_ca" | "fr_ch" => "Français",
		"ga" => "Gaeilge",
		"gl" => "Galego",
		"he" => "עברית",
		"hi" => "हिन्दी",
		"hr" => "Hrvatski",
		"hu" => "Magyar",
		"hy" => "Հայերեն",
		"id" => "Indonesia",
		"is" => "Íslenska",
		"it" => "Italiano",
		"ja" => "日本語",
		"ka" => "ქართული",
		"ko" => "한국어",
		"lt" => "Lietuvių",
		"lv" => "Latviešu",
		"mk" => "Македонски",
		"ms" => "Bahasa Melayu",
		"nb" => "Norsk bokmål",
		"nl" => "Nederlands",
		"nn" => "Norsk nynorsk",
		"pl" => "Polski",
		"pt" => "Português",
		"pt_br" => "Português (Brasil)",
		"ro" => "Română",
		"ru" => "Русский",
		"sk" => "Slovenčina",
		"sl" => "Slovenščina",
		"sq" => "Shqip",
		"sr" => "Српски",
		"sv" => "Svenska",
		"th" => "ภาษาไทย",
		"tr" => "Türkçe",
		"uk" => "Українська",
		"vi" => "Tiếng Việt",
		"zh" => "中文",
		"zh_cn" => "中文（简体）",
		"zh_tw" => "中文（繁體）",
		_ => code,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn language_name_known() {
		assert_eq!(language_name("de"), "Deutsch");
		assert_eq!(language_name("pt_br"), "Português (Brasil)");
		assert_eq!(language_name("pt_BR"), "Português (Brasil)");
		assert_eq!(language_name("zh_CN"), "中文（简体）");
	}

	#[test]
	fn language_name_unknown_returns_code() {
		assert_eq!(language_name("xx"), "xx");
		assert_eq!(language_name("tok"), "tok");
	}

	fn available(codes: &[&str]) -> Vec<LanguageInfo> {
		codes.iter().map(|c| LanguageInfo { code: (*c).to_string(), name: (*c).to_string() }).collect()
	}

	#[test]
	fn an_exact_code_wins() {
		let langs = available(&["en", "de", "zh_CN"]);
		assert_eq!(best_available_language("zh_CN", &langs).as_deref(), Some("zh_CN"));
	}

	#[test]
	fn the_same_code_in_another_case_still_matches() {
		let langs = available(&["en", "pt_br"]);
		assert_eq!(best_available_language("pt_BR", &langs).as_deref(), Some("pt_br"));
		assert_eq!(best_available_language("pt-BR", &langs).as_deref(), Some("pt_br"));
	}

	#[test]
	fn a_region_the_catalogs_do_not_have_falls_back_to_the_language() {
		let langs = available(&["en", "de"]);
		assert_eq!(best_available_language("de_DE", &langs).as_deref(), Some("de"));
	}

	#[test]
	fn a_script_in_the_system_locale_does_not_hide_the_matching_catalog() {
		let langs = available(&["en", "zh_TW", "zh_CN"]);
		assert_eq!(best_available_language("zh-Hans-CN", &langs).as_deref(), Some("zh_CN"));
		assert_eq!(best_available_language("zh-Hant-TW", &langs).as_deref(), Some("zh_TW"));
	}

	#[test]
	fn the_most_specific_matching_catalog_wins() {
		let langs = available(&["zh", "zh_CN"]);
		assert_eq!(best_available_language("zh-Hans-CN", &langs).as_deref(), Some("zh_CN"));
	}

	#[test]
	fn another_region_of_the_same_language_is_better_than_english() {
		let langs = available(&["en", "zh_CN"]);
		assert_eq!(best_available_language("zh_TW", &langs).as_deref(), Some("zh_CN"));
	}

	#[test]
	fn a_language_with_no_catalog_matches_nothing() {
		let langs = available(&["en", "de"]);
		assert_eq!(best_available_language("ja_JP", &langs), None);
	}
}
