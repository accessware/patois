//! Procedural macros for [patois](https://docs.rs/patois).
//!
//! Depend on `patois` and use these macros through its re-exports, because their output refers to
//! items in `patois`.

use std::{
	collections::HashSet,
	env, fs,
	path::{Path, PathBuf},
};

use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::quote;

/// Embeds the compiled catalogs of the calling crate in the binary.
///
/// Reads every `<crate_root>/locale/<lang>/LC_MESSAGES/<crate_name>.mo` file at compile time. Use
/// the macro once, at module level, in the app crate. Generate the files in a build script with
/// `patois_build::compile_translations`, so that they exist when the macro runs.
///
/// The catalogs are stored deflated, and each one is inflated only when its locale is first used,
/// so a binary with many languages holds one of them in memory at a time.
///
/// # Panics
///
/// Stops compilation with an error if a catalog can't be read, or if Cargo didn't set
/// `CARGO_MANIFEST_DIR` and `CARGO_PKG_NAME`.
#[proc_macro]
pub fn embed_domain(_input: TokenStream) -> TokenStream {
	let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set during macro expansion");
	let pkg_name = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME not set during macro expansion");
	let locale_dir = Path::new(&manifest_dir).join("locale");
	let mut file_entries = vec![];
	if locale_dir.exists()
		&& let Ok(entries) = fs::read_dir(&locale_dir)
	{
		let mut locales: Vec<_> = entries.flatten().collect();
		locales.sort_by_key(std::fs::DirEntry::file_name);
		for entry in locales {
			let locale_name = entry.file_name().to_string_lossy().to_string();
			let mo_path = entry.path().join("LC_MESSAGES").join(format!("{pkg_name}.mo"));
			if mo_path.exists() {
				let catalog = embedded_catalog(&mo_path);
				file_entries.push(quote! { (#locale_name, #catalog) });
			}
		}
	}
	quote! {
		::patois::inventory::submit! {
			::patois::EmbeddedDomain {
				name: env!("CARGO_PKG_NAME"),
				files: &[ #(#file_entries),* ],
			}
		}
	}
	.into()
}

/// Embeds the `wxstd` catalogs of wxWidgets for the languages that the calling crate ships, and
/// generates a `WxStdCatalogLoader` type that serves them.
///
/// wxWidgets has its own translations for stock strings, such as the labels on standard buttons.
/// The `wxdragon-sys` build puts them under `target/<profile>`, which doesn't exist next to an
/// installed app, so this macro copies them into the binary. It embeds only the languages under
/// `<crate_root>/locale/`, so the binary doesn't carry languages that the app doesn't have.
///
/// Use the macro once, at module level, in an app crate that depends on `wxdragon` and on `patois`
/// with the `wx` feature. The crate's build script must call `patois_build::compile_translations`,
/// which gives this macro the build profile that it needs to find the catalogs.
///
/// If the catalogs don't exist, for example because wxWidgets was built without `msgfmt`, the
/// macro embeds nothing and stock strings stay in English.
///
/// # Panics
///
/// Stops compilation with an error if a catalog can't be read, or if Cargo didn't set
/// `CARGO_MANIFEST_DIR`.
#[proc_macro]
pub fn embed_wx_translations(_input: TokenStream) -> TokenStream {
	let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set during macro expansion");
	let app_langs = shipped_languages(Path::new(&manifest_dir));
	let catalogs = wx_locale_dir().map_or_else(Vec::new, |locale_dir| matching_wx_catalogs(&locale_dir, &app_langs));
	let file_entries = catalogs.iter().map(|(lang, mo_path)| {
		let catalog = embedded_catalog(mo_path);
		quote! { (#lang, #catalog) }
	});
	quote! {
		/// Serves the `wxstd` catalogs that `patois::embed_wx_translations!()` embedded.
		pub struct WxStdCatalogLoader;

		impl WxStdCatalogLoader {
			const CATALOGS: &'static [(&'static str, &'static [u8])] = &[ #(#file_entries),* ];

			// wxWidgets asks for the versioned domain, such as `wxstd-3.3`, and then for `wxstd`.
			fn is_wxstd_domain(domain: &str) -> bool {
				domain == "wxstd" || domain.starts_with("wxstd-")
			}
		}

		impl ::wxdragon::translations::TranslationsLoader for WxStdCatalogLoader {
			fn load_catalog(&self, domain: &str, lang: &str) -> ::std::option::Option<::std::borrow::Cow<'_, [u8]>> {
				if !Self::is_wxstd_domain(domain) {
					return None;
				}
				Self::CATALOGS
					.iter()
					.find(|(l, _)| l.eq_ignore_ascii_case(lang))
					.and_then(|(_, bytes)| ::patois::__inflate(bytes))
					.map(::std::borrow::Cow::Owned)
			}

			fn available_translations(&self, domain: &str) -> ::std::vec::Vec<::std::string::String> {
				if !Self::is_wxstd_domain(domain) {
					return ::std::vec::Vec::new();
				}
				Self::CATALOGS.iter().map(|(l, _)| l.to_string()).collect()
			}
		}
	}
	.into()
}

// The `include_bytes!` is never read, so it adds nothing to the binary. It's there because Cargo
// rebuilds a crate when an included file changes, and can't see this macro read the catalog.
fn embedded_catalog(path: &Path) -> TokenStream2 {
	let path_str = path.to_string_lossy().to_string();
	let bytes = fs::read(path).unwrap_or_else(|e| panic!("couldn't read {}: {e}", path.display()));
	let deflated = Literal::byte_string(&miniz_oxide::deflate::compress_to_vec(&bytes, 10));
	quote! {{
		const _: &[u8] = include_bytes!(#path_str);
		const B: &[u8] = #deflated;
		B
	}}
}

fn shipped_languages(manifest_dir: &Path) -> HashSet<String> {
	let locale_dir = manifest_dir.join("locale");
	let mut langs = HashSet::new();
	if let Ok(entries) = fs::read_dir(&locale_dir) {
		for entry in entries.flatten() {
			if entry.path().is_dir() {
				langs.insert(entry.file_name().to_string_lossy().to_lowercase());
			}
		}
	}
	langs
}

fn wx_locale_dir() -> Option<PathBuf> {
	Some(target_profile_dir()?.join("wxdragon_sys_cmake_build").join("share").join("locale"))
}

// `OUT_DIR` is `target/<profile>/build/<crate>-<hash>/out`.
fn target_profile_dir() -> Option<PathBuf> {
	let profile = env::var("PROFILE").ok()?;
	if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
		let mut dir = PathBuf::from(target_dir);
		dir.push(profile);
		return Some(dir);
	}
	let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
	out_dir.ancestors().nth(3).map(Path::to_path_buf)
}

// wxWidgets names a language `pt_BR` where an app may name it `pt_br`, so `app_langs` is lowercase.
fn matching_wx_catalogs(locale_dir: &Path, app_langs: &HashSet<String>) -> Vec<(String, PathBuf)> {
	let Ok(entries) = fs::read_dir(locale_dir) else {
		return Vec::new();
	};
	let mut lang_entries: Vec<_> = entries.flatten().collect();
	lang_entries.sort_by_key(std::fs::DirEntry::file_name);
	let mut catalogs = Vec::new();
	for entry in lang_entries {
		let lang = entry.file_name().to_string_lossy().to_string();
		if !app_langs.contains(&lang.to_lowercase()) {
			continue;
		}
		let lc_messages = entry.path().join("LC_MESSAGES");
		let Ok(files) = fs::read_dir(&lc_messages) else { continue };
		let mut mo_files: Vec<PathBuf> = files
			.flatten()
			.map(|f| f.path())
			.filter(|p| {
				p.extension().and_then(|e| e.to_str()) == Some("mo")
					&& p.file_stem().and_then(|s| s.to_str()).is_some_and(|s| s.starts_with("wxstd"))
			})
			.collect();
		mo_files.sort();
		if let Some(mo_path) = mo_files.into_iter().next() {
			catalogs.push((lang, mo_path));
		}
	}
	catalogs
}

#[cfg(test)]
mod tests {
	use super::*;

	struct ScratchDir(PathBuf);

	impl ScratchDir {
		fn new(name: &str) -> Self {
			let path = env::temp_dir().join(format!("patois_macros_test_{name}_{}", std::process::id()));
			let _ = fs::remove_dir_all(&path);
			fs::create_dir_all(&path).expect("create scratch dir");
			Self(path)
		}

		fn path(&self) -> &Path {
			&self.0
		}
	}

	impl Drop for ScratchDir {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.0);
		}
	}

	fn write_mo(dir: &Path, lang: &str, filename: &str) {
		let lc_messages = dir.join(lang).join("LC_MESSAGES");
		fs::create_dir_all(&lc_messages).expect("create LC_MESSAGES dir");
		fs::write(lc_messages.join(filename), b"fake mo bytes").expect("write fake .mo file");
	}

	#[test]
	fn shipped_languages_lists_locale_subdirectories() {
		let scratch = ScratchDir::new("shipped_languages");
		let locale_dir = scratch.path().join("locale");
		fs::create_dir_all(locale_dir.join("de")).unwrap();
		fs::create_dir_all(locale_dir.join("pt_BR")).unwrap();
		fs::write(locale_dir.join("README.md"), b"not a language").unwrap();
		let langs = shipped_languages(scratch.path());
		assert_eq!(langs, HashSet::from(["de".to_string(), "pt_br".to_string()]));
	}

	#[test]
	fn shipped_languages_empty_when_locale_dir_missing() {
		let scratch = ScratchDir::new("shipped_languages_missing");
		assert!(shipped_languages(scratch.path()).is_empty());
	}

	#[test]
	fn matching_wx_catalogs_filters_by_app_language_case_insensitively() {
		let scratch = ScratchDir::new("matching_catalogs");
		write_mo(scratch.path(), "de", "wxstd-3.3.mo");
		write_mo(scratch.path(), "pt_BR", "wxstd-3.3.mo");
		write_mo(scratch.path(), "af", "wxstd-3.3.mo");
		let app_langs = HashSet::from(["de".to_string(), "pt_br".to_string()]);
		let mut catalogs = matching_wx_catalogs(scratch.path(), &app_langs);
		catalogs.sort();
		assert_eq!(catalogs.len(), 2);
		assert_eq!(catalogs[0].0, "de");
		assert_eq!(catalogs[1].0, "pt_BR");
	}

	#[test]
	fn matching_wx_catalogs_picks_lowest_sorted_mo_when_multiple_present() {
		let scratch = ScratchDir::new("matching_catalogs_multi");
		write_mo(scratch.path(), "de", "wxstd-3.3.mo");
		write_mo(scratch.path(), "de", "wxstd-3.2.mo");
		let app_langs = HashSet::from(["de".to_string()]);
		let catalogs = matching_wx_catalogs(scratch.path(), &app_langs);
		assert_eq!(catalogs.len(), 1);
		assert!(catalogs[0].1.ends_with("wxstd-3.2.mo"));
	}

	#[test]
	fn matching_wx_catalogs_ignores_non_wxstd_mo_files() {
		let scratch = ScratchDir::new("matching_catalogs_ignore");
		write_mo(scratch.path(), "de", "somethingelse.mo");
		let app_langs = HashSet::from(["de".to_string()]);
		assert!(matching_wx_catalogs(scratch.path(), &app_langs).is_empty());
	}

	#[test]
	fn matching_wx_catalogs_empty_when_locale_dir_missing() {
		let scratch = ScratchDir::new("matching_catalogs_missing");
		let missing = scratch.path().join("does_not_exist");
		let app_langs = HashSet::from(["de".to_string()]);
		assert!(matching_wx_catalogs(&missing, &app_langs).is_empty());
	}
}
