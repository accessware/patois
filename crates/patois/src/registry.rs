use std::{
	collections::HashMap,
	fs::{self, File},
	path::{Path, PathBuf},
	sync::{Mutex, OnceLock},
};

use crate::{EmbeddedDomain, catalog::Catalog};

static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();

pub fn global() -> &'static Mutex<Registry> {
	REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

#[derive(Default)]
pub struct Registry {
	paths: HashMap<String, PathBuf>,
	default_domain: Option<String>,
	locale: String,
	cache: HashMap<String, Option<Catalog>>,
}

impl Registry {
	pub fn register_path(&mut self, domain: &str, path: PathBuf) {
		if self.paths.get(domain) == Some(&path) {
			return;
		}
		self.cache.remove(domain);
		self.paths.insert(domain.to_string(), path);
	}

	pub fn set_default_domain(&mut self, domain: &str) {
		self.default_domain = Some(domain.to_string());
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

	pub fn translate(&mut self, msgid: &str) -> String {
		self.catalog().map_or(msgid, |catalog| catalog.gettext(msgid)).to_string()
	}

	pub fn translate_plural(&mut self, singular: &str, plural: &str, n: u64) -> String {
		match self.catalog() {
			Some(catalog) => catalog.ngettext(singular, plural, n),
			None if n == 1 => singular,
			None => plural,
		}
		.to_string()
	}

	fn catalog(&mut self) -> Option<&Catalog> {
		let domain = self.default_domain.clone()?;
		if !self.cache.contains_key(&domain) {
			let catalog = self.load_catalog(&domain);
			self.cache.insert(domain.clone(), catalog);
		}
		self.cache.get(&domain)?.as_ref()
	}

	fn load_catalog(&self, domain: &str) -> Option<Catalog> {
		locale_chain(&self.locale).into_iter().find_map(|locale| {
			inventory::iter::<EmbeddedDomain>
				.into_iter()
				.filter(|embedded| embedded.name == domain)
				.find_map(|embedded| embedded_catalog(embedded.files, locale))
				.or_else(|| catalog_on_disk(self.paths.get(domain)?, domain, locale))
		})
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

/// Converts `en-US` and `en_us` to `en_US`, and `zh-Hans-CN` to `zh_HANS_CN`.
pub fn normalize_locale(locale: &str) -> String {
	let s = locale.replace('-', "_");
	match s.split_once('_') {
		Some((lang, region)) => format!("{}_{}", lang.to_lowercase(), region.to_uppercase()),
		None => s.to_lowercase(),
	}
}

fn locale_chain(locale: &str) -> Vec<&str> {
	let mut chain = vec![locale];
	let mut rest = locale;
	while let Some((shorter, _)) = rest.rsplit_once('_') {
		chain.push(shorter);
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

	// A `.mo` file opens with a magic number that isn't a valid deflate stream, so an undeflated
	// catalog is skipped rather than misread.
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
		registry.set_default_domain("app");
		registry.set_locale("pt_br");
		let translated = registry.translate("Hello");
		fs::remove_dir_all(&base).unwrap();
		assert_eq!(translated, "Olá");
	}

	#[test]
	fn nothing_is_translated_without_a_domain() {
		let mut registry = Registry::default();
		registry.set_locale("de");
		assert_eq!(registry.translate("Hello"), "Hello");
		assert_eq!(registry.translate_plural("file", "files", 1), "file");
		assert_eq!(registry.translate_plural("file", "files", 2), "files");
	}
}
