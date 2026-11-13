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

mod bake;

/// Embeds the compiled catalogs of the calling crate in the binary.
///
/// Reads every `<crate_root>/locale/<lang>/LC_MESSAGES/<crate_name>.mo` file at compile time. Use
/// the macro once, at module level, in the app crate. Generate the files in a build script with
/// `patois_build::compile_translations`, so that they exist when the macro runs.
///
/// The catalogs are written out as Rust statics: one sorted list of msgids that every locale
/// shares, and for each locale its translations in one string with where each one starts. Nothing
/// is parsed, checked or copied at runtime, and each msgid is stored once rather than once per
/// locale. Nothing is compressed either, since the installer or archive compresses the whole
/// binary, and does it better.
///
/// # Panics
///
/// Stops compilation with an error if a catalog can't be read or isn't a valid `.mo` file, or if
/// Cargo didn't set `CARGO_MANIFEST_DIR` and `CARGO_PKG_NAME`.
#[proc_macro]
pub fn embed_domain(_input: TokenStream) -> TokenStream {
	let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set during macro expansion");
	let pkg_name = env::var("CARGO_PKG_NAME").expect("CARGO_PKG_NAME not set during macro expansion");
	let locale_dir = Path::new(&manifest_dir).join("locale");
	let mut catalogs = vec![];
	let mut tracked = vec![];
	if locale_dir.exists()
		&& let Ok(entries) = fs::read_dir(&locale_dir)
	{
		let mut locales: Vec<_> = entries.flatten().collect();
		locales.sort_by_key(std::fs::DirEntry::file_name);
		for entry in locales {
			let locale_name = entry.file_name().to_string_lossy().to_string();
			let mo_path = entry.path().join("LC_MESSAGES").join(format!("{pkg_name}.mo"));
			if mo_path.exists() {
				let bytes = fs::read(&mo_path).unwrap_or_else(|e| panic!("couldn't read {}: {e}", mo_path.display()));
				catalogs.push((locale_name, bytes));
				tracked.push(rebuild_when_changed(&mo_path));
			}
		}
	}
	let baked = bake::bake(&catalogs).unwrap_or_else(|e| panic!("{e}"));
	let msgids = &baked.msgids;
	let locales = baked.locales.iter().map(|locale| {
		let (code, header, text, starts) = (&locale.code, &locale.header, &locale.text, &locale.starts);
		quote! {
			::patois::EmbeddedLocale { code: #code, header: #header, text: #text, starts: &[ #(#starts),* ] }
		}
	});
	quote! {
		#(#tracked)*
		::patois::inventory::submit! {
			::patois::EmbeddedDomain {
				name: env!("CARGO_PKG_NAME"),
				msgids: &[ #(#msgids),* ],
				locales: &[ #(#locales),* ],
			}
		}
	}
	.into()
}

// The `include_bytes!` is never read, so it adds nothing to the binary. It's there because Cargo
// rebuilds a crate when an included file changes, and can't see this macro read the catalog.
fn rebuild_when_changed(path: &Path) -> TokenStream2 {
	let path_str = path.to_string_lossy().to_string();
	quote! { const _: &[u8] = include_bytes!(#path_str); }
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
					.map(|&(_, bytes)| ::std::borrow::Cow::Borrowed(bytes))
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
	let bytes = Literal::byte_string(&bytes);
	quote! {{
		const _: &[u8] = include_bytes!(#path_str);
		const B: &[u8] = #bytes;
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
	let var = |name| env::var_os(name).map(PathBuf::from);
	locale_dir_from(var("WXDRAGON_SYS_BUILD_DIR"), &env::var("PROFILE").ok()?, var("OUT_DIR"), var("CARGO_TARGET_DIR"))
}

// Where wxdragon-sys builds: `WXDRAGON_SYS_BUILD_DIR` when it's set, else the profile directory it
// finds from `OUT_DIR`, `target/<profile>/build/<crate>-<hash>/out` or, for a cross build,
// `target/<triple>/<profile>/...`. `CARGO_TARGET_DIR` is only the fallback, since it leaves out the
// triple. The result is absolute, since `include_bytes!` reads a relative path from the source
// file, not from where the compiler runs as `fs::read` does.
fn locale_dir_from(
	build_dir: Option<PathBuf>,
	profile: &str,
	out_dir: Option<PathBuf>,
	target_dir: Option<PathBuf>,
) -> Option<PathBuf> {
	let build_dir = build_dir.or_else(|| {
		let profile_dir = match out_dir {
			Some(out_dir) => {
				out_dir.ancestors().find(|dir| dir.file_name().is_some_and(|name| name == profile))?.to_path_buf()
			}
			None => target_dir?.join(profile),
		};
		Some(profile_dir.join("wxdragon_sys_cmake_build"))
	})?;
	std::path::absolute(build_dir.join("share").join("locale")).ok()
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

	fn locale_under(dir: &Path) -> PathBuf {
		dir.join("wxdragon_sys_cmake_build").join("share").join("locale")
	}

	#[test]
	fn the_catalogs_are_found_beside_the_build_out_dir_names() {
		let target = env::temp_dir().join("target");
		let out_dir = target.join("release").join("build").join("app-1234").join("out");
		let found = locale_dir_from(None, "release", Some(out_dir), None);
		assert_eq!(found, Some(locale_under(&target.join("release"))));
	}

	/// Cargo 1.100 nests `OUT_DIR` one deeper, `build/<crate>/<hash>/out`.
	#[test]
	fn the_catalogs_are_found_from_cargo_1_100s_out_dir() {
		let target = env::temp_dir().join("target");
		let out_dir = target.join("release").join("build").join("app").join("1234").join("out");
		let found = locale_dir_from(None, "release", Some(out_dir), None);
		assert_eq!(found, Some(locale_under(&target.join("release"))));
	}

	#[test]
	fn a_cross_build_keeps_its_target_triple() {
		let target = env::temp_dir().join("target");
		let profile = target.join("aarch64-pc-windows-msvc").join("release");
		let out_dir = profile.join("build").join("app-1234").join("out");
		// `CARGO_TARGET_DIR` leaves the triple out, so it mustn't win over `OUT_DIR`.
		let found = locale_dir_from(None, "release", Some(out_dir), Some(target));
		assert_eq!(found, Some(locale_under(&profile)));
	}

	#[test]
	fn a_relative_target_dir_becomes_absolute() {
		let found = locale_dir_from(None, "debug", None, Some(PathBuf::from("target").join("udeps")))
			.expect("a locale directory");
		assert!(found.is_absolute(), "{}", found.display());
		assert!(found.ends_with(locale_under(&Path::new("target").join("udeps").join("debug"))));
	}

	#[test]
	fn wxdragon_sys_build_dir_moves_the_catalogs_with_it() {
		let build = env::temp_dir().join("wx-build");
		let out_dir = env::temp_dir().join("target").join("release").join("build").join("app").join("out");
		let found = locale_dir_from(Some(build.clone()), "release", Some(out_dir), None);
		assert_eq!(found, Some(build.join("share").join("locale")));
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
		assert_eq!(matching_wx_catalogs(scratch.path(), &app_langs), [] as [(String, PathBuf); 0]);
	}

	#[test]
	fn matching_wx_catalogs_empty_when_locale_dir_missing() {
		let scratch = ScratchDir::new("matching_catalogs_missing");
		let missing = scratch.path().join("does_not_exist");
		let app_langs = HashSet::from(["de".to_string()]);
		assert_eq!(matching_wx_catalogs(&missing, &app_langs), [] as [(String, PathBuf); 0]);
	}
}
