use std::{
	collections::HashMap,
	fs::{self, File},
	path::{Path, PathBuf},
	sync::{Mutex, OnceLock},
};

use crate::{EmbeddedDomain, catalog::Catalog};

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

pub(crate) fn global() -> &'static Mutex<Registry> {
	REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[derive(Default)]
pub(crate) struct Registry {
	/// App domain paths (populated by `init()`). Library domains use embedded bytes instead.
	paths: HashMap<String, PathBuf>,
	default_domain: Option<String>,
	locale: String,
	cache: HashMap<(String, String), Option<Catalog>>,
}

impl Registry {
	pub(crate) fn register_path(&mut self, domain: &str, path: PathBuf) {
		if self.paths.get(domain) == Some(&path) {
			return;
		}
		self.cache.retain(|(_, d), _| d != domain);
		self.paths.insert(domain.to_string(), path);
	}

	pub fn set_default_domain(&mut self, domain: &str) {
		self.default_domain = Some(domain.to_string());
	}

	pub fn default_domain(&self) -> Option<&str> {
		self.default_domain.as_deref()
	}

	pub fn set_locale(&mut self, locale_id: &str) {
		let normalized = normalize_locale(locale_id);
		if self.locale != normalized {
			self.locale = normalized;
			self.cache.clear();
		}
	}

	pub fn locale(&self) -> &str {
		&self.locale
	}

	pub fn translate(&mut self, domain: &str, msgid: &str) -> String {
		self.ensure_catalog(domain);
		let key = (self.locale.clone(), domain.to_string());
		if let Some(Some(catalog)) = self.cache.get(&key) {
			return catalog.gettext(msgid).to_string();
		}
		// Fall back to the default domain when the specific domain has no catalog.
		if let Some(default) = self.default_domain.clone()
			&& default != domain
		{
			self.ensure_catalog(&default);
			let fallback_key = (self.locale.clone(), default);
			if let Some(Some(catalog)) = self.cache.get(&fallback_key) {
				return catalog.gettext(msgid).to_string();
			}
		}
		msgid.to_string()
	}

	pub fn translate_plural(&mut self, domain: &str, singular: &str, plural: &str, n: u64) -> String {
		self.ensure_catalog(domain);
		let key = (self.locale.clone(), domain.to_string());
		if let Some(Some(catalog)) = self.cache.get(&key) {
			return catalog.ngettext(singular, plural, n).to_string();
		}
		if let Some(default) = self.default_domain.clone()
			&& default != domain
		{
			self.ensure_catalog(&default);
			let fallback_key = (self.locale.clone(), default);
			if let Some(Some(catalog)) = self.cache.get(&fallback_key) {
				return catalog.ngettext(singular, plural, n).to_string();
			}
		}
		if n == 1 { singular.to_string() } else { plural.to_string() }
	}

	fn ensure_catalog(&mut self, domain: &str) {
		let locale = self.locale.clone();
		let key = (locale.clone(), domain.to_string());
		if !self.cache.contains_key(&key) {
			let catalog = self.load_catalog(domain, &locale);
			self.cache.insert(key, catalog);
		}
	}

	fn load_catalog(&self, domain: &str, locale: &str) -> Option<Catalog> {
		for loc in locale_chain(locale) {
			// Embedded domains (libraries using embed_domain!()) take priority.
			for ed in inventory::iter::<EmbeddedDomain> {
				if ed.name == domain
					&& let Some(catalog) = embedded_catalog(ed.files, &loc)
				{
					return Some(catalog);
				}
			}
			// Path-based domains (the app's own strings via init()).
			if let Some(base) = self.paths.get(domain)
				&& let Some(catalog) = catalog_on_disk(base, domain, &loc)
			{
				return Some(catalog);
			}
		}
		None
	}
}

// Catalog directories keep whatever name a project gave them, such as `pt_br`, so both sides of
// each comparison below go through `normalize_locale`.
fn embedded_catalog(files: &[(&str, &[u8])], locale: &str) -> Option<Catalog> {
	files
		.iter()
		.filter(|(code, _)| normalize_locale(code) == locale)
		.find_map(|(_, deflated)| Catalog::parse(crate::__inflate(deflated)?.as_slice()).ok())
}

fn catalog_on_disk(base: &Path, domain: &str, locale: &str) -> Option<Catalog> {
	fs::read_dir(base)
		.ok()?
		.flatten()
		.filter(|entry| normalize_locale(&entry.file_name().to_string_lossy()) == locale)
		.find_map(|entry| {
			Catalog::parse(File::open(entry.path().join("LC_MESSAGES").join(format!("{domain}.mo"))).ok()?).ok()
		})
}

/// Normalize locale strings: "en-US" and "en_us" both become "en_US".
pub(crate) fn normalize_locale(locale: &str) -> String {
	let s = locale.replace('-', "_");
	match s.split_once('_') {
		Some((lang, region)) => format!("{}_{}", lang.to_lowercase(), region.to_uppercase()),
		None => s.to_lowercase(),
	}
}

fn locale_chain(locale: &str) -> Vec<String> {
	let mut chain = vec![locale.to_string()];
	let mut rest = locale;
	while let Some((shorter, _)) = rest.rsplit_once('_') {
		chain.push(shorter.to_string());
		rest = shorter;
	}
	chain
}

#[cfg(test)]
mod tests {
	use std::env;

	use super::*;
	use crate::catalog::tests::make_mo;

	fn deflated_catalog(msgstr: &str) -> Vec<u8> {
		miniz_oxide::deflate::compress_to_vec(&make_mo(&[("Hello", msgstr)]), 10)
	}

	#[test]
	fn normalize_various_forms() {
		assert_eq!(normalize_locale("en-US"), "en_US");
		assert_eq!(normalize_locale("DE_de"), "de_DE");
		assert_eq!(normalize_locale("fr"), "fr");
		assert_eq!(normalize_locale("zh-Hans-CN"), "zh_HANS_CN");
	}

	#[test]
	fn inflate_undoes_the_macros_deflate() {
		let catalog = b"msgid and msgstr bytes ".repeat(20);
		assert_eq!(crate::__inflate(&miniz_oxide::deflate::compress_to_vec(&catalog, 10)), Some(catalog));
	}

	// A .mo file that was never deflated opens with its magic number, which is not a valid deflate stream, so it is skipped rather than misread.
	#[test]
	fn inflate_rejects_a_catalog_that_was_never_deflated() {
		assert_eq!(crate::__inflate(b"\xDE\x12\x04\x95"), None);
	}

	#[test]
	fn locale_chain_expands_region() {
		assert_eq!(locale_chain("de_DE"), vec!["de_DE", "de"]);
		assert_eq!(locale_chain("fr"), vec!["fr"]);
		assert_eq!(locale_chain("zh_HANS_CN"), vec!["zh_HANS_CN", "zh_HANS", "zh"]);
	}

	#[test]
	fn an_embedded_catalog_matches_whatever_its_directory_is_called() {
		let deflated = deflated_catalog("Olá");
		let files: [(&str, &[u8]); 1] = [("pt_br", &deflated)];
		let catalog = embedded_catalog(&files, &normalize_locale("pt-BR")).unwrap();
		assert_eq!(catalog.gettext("Hello"), "Olá");
		assert!(embedded_catalog(&files, "de").is_none());
	}

	#[test]
	fn a_catalog_on_disk_matches_whatever_its_directory_is_called() {
		let base = env::temp_dir().join(format!("patois-registry-test-{}", std::process::id()));
		let dir = base.join("pt_br").join("LC_MESSAGES");
		fs::create_dir_all(&dir).unwrap();
		fs::write(dir.join("app.mo"), make_mo(&[("Hello", "Olá")])).unwrap();
		let mut registry = Registry::default();
		registry.register_path("app", base.clone());
		registry.set_locale("pt_br");
		let translated = registry.translate("app", "Hello");
		fs::remove_dir_all(&base).unwrap();
		assert_eq!(translated, "Olá");
	}
}
