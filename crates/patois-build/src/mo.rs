use std::{
	env, fs,
	path::{Path, PathBuf},
	process::Command,
};

/// Compiles each `.po` file in `po_dir` to `<locale_dir>/<lang>/LC_MESSAGES/<domain>.mo`, where
/// `<domain>` is the name of the crate.
///
/// Call this function from a build script. Relative paths start at the crate root. The function
/// tells Cargo to run the build script again when a catalog changes.
///
/// Problems such as a missing `msgfmt` are reported as Cargo warnings, so a build without gettext
/// still succeeds, without translations.
pub fn compile_translations(po_dir: impl AsRef<Path>, locale_dir: impl AsRef<Path>) {
	let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
	let abs = |p: &Path| if p.is_absolute() { p.to_path_buf() } else { manifest_dir.join(p) };
	let po_dir = abs(po_dir.as_ref());
	let locale_dir = abs(locale_dir.as_ref());
	let domain = env::var("CARGO_PKG_NAME").unwrap_or_default();
	println!("cargo:rerun-if-changed={}", po_dir.display());
	println!("cargo:rerun-if-changed={}", locale_dir.display());
	// `embed_wx_translations!` needs the profile to find catalogs, and Cargo sets it only for build
	// scripts.
	if let Ok(profile) = env::var("PROFILE") {
		println!("cargo:rustc-env=PROFILE={profile}");
	}
	let entries = match fs::read_dir(&po_dir) {
		Ok(e) => e,
		Err(e) => {
			println!("cargo:warning=patois-build: could not read {}: {e}", po_dir.display());
			return;
		}
	};
	for entry in entries {
		let path = match entry {
			Ok(e) => e.path(),
			Err(e) => {
				println!("cargo:warning=patois-build: {e}");
				continue;
			}
		};
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		println!("cargo:rerun-if-changed={}", path.display());
		let out_dir = locale_dir.join(&lang).join("LC_MESSAGES");
		if let Err(e) = fs::create_dir_all(&out_dir) {
			println!("cargo:warning=patois-build: could not create {}: {e}", out_dir.display());
			continue;
		}
		run_msgfmt(&path, &out_dir.join(format!("{domain}.mo")));
	}
}

fn run_msgfmt(input: &Path, output: &Path) {
	// `fuzzy` marks a machine translation that no person has reviewed, as `PoDocument::apply`
	// writes it. msgfmt drops fuzzy entries by default, and these would then never reach the
	// app. The hash table only speeds up GNU gettext's own lookup; patois binary-searches the
	// sorted strings, so the table would only take up space in the binary.
	match Command::new("msgfmt").arg("--use-fuzzy").arg("--no-hash").arg(input).arg("-o").arg(output).status() {
		Ok(s) if s.success() => {}
		Ok(s) => println!("cargo:warning=patois-build: msgfmt exited with {s} compiling {}", input.display()),
		Err(e) => println!("cargo:warning=patois-build: msgfmt not available ({e}); install gettext tools"),
	}
}
