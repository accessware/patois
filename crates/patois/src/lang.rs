use crate::{available_locales, locale, registry::normalize_locale, set_locale};

/// A language available for the app, with its code and native display name.
#[derive(Debug, Clone)]
pub struct LanguageInfo {
	/// BCP 47 / gettext locale code as stored in the `locale/` directory (e.g. `"de"`, `"pt_br"`).
	pub code: String,
	/// Native name suitable for display in a language-selector UI (e.g. `"Deutsch"`).
	pub name: String,
}

/// Manages runtime language selection for a patois domain.
///
/// # Example
///
/// ```rust,no_run
/// patois::embed_domain!();
///
/// fn main() {
///     patois::init_auto(env!("CARGO_PKG_NAME"));
///     let mgr = patois::LanguageManager::new(env!("CARGO_PKG_NAME"));
///     for lang in mgr.available() {
///         println!("{}: {}", lang.code, lang.name);
///     }
///     mgr.set("de");
/// }
/// ```
pub struct LanguageManager {
	domain: String,
}

impl LanguageManager {
	/// Create a manager for the given patois domain.
	pub fn new(domain: &str) -> Self {
		Self { domain: domain.to_string() }
	}

	/// All available languages: English is always first, the rest are sorted by native name.
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

	/// Returns the currently active locale code.
	pub fn current(&self) -> String {
		let active = locale();
		available_locales(&self.domain)
			.into_iter()
			.find(|code| normalize_locale(code) == active)
			.map_or(active, str::to_string)
	}

	/// Switch to `code`. Returns `false` if the code is not in the available list (English always accepted).
	pub fn set(&self, code: &str) -> bool {
		let wanted = normalize_locale(code);
		if wanted == "en" || available_locales(&self.domain).iter().any(|c| normalize_locale(c) == wanted) {
			set_locale(code);
			true
		} else {
			false
		}
	}

	/// Detect the operating system's preferred locale, falling back to `"en"`.
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

/// Returns the native display name for a locale code, or the code itself if not recognised.
///
/// Matching is done on the raw code as stored in your `locale/` directory.
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
