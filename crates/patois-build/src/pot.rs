use std::{
	collections::HashSet,
	env, error, fs,
	path::{Path, PathBuf},
	process::{self, Command},
};

use crate::{
	entries::{collect_pot_entries_ordered, collect_pot_msgids, pot_entry_block},
	sanitize_rust,
};

/// Regenerates `<po_dir>/<package_name>.pot` from the Rust files in `source_dirs`.
///
/// Unlike [`gen_pot`], this function doesn't run `cargo`, so it's safe to call from a build script.
/// It needs `xgettext` on `PATH`.
///
/// # Errors
///
/// Returns an error if `xgettext` is missing or fails, or if a file can't be read or written.
pub fn gen_pot_from_dirs(
	source_dirs: &[impl AsRef<Path>],
	po_dir: impl AsRef<Path>,
	package_name: &str,
	package_version: &str,
) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	if Command::new("xgettext").arg("--version").output().is_err() {
		return Err("xgettext not found; install gettext tools".into());
	}
	let mut files: Vec<PathBuf> = Vec::new();
	for dir in source_dirs {
		// Sort within each directory only. Directories can be in unrelated places, such as a
		// checkout and a registry cache, whose relative order differs between machines.
		let mut dir_files: Vec<PathBuf> = Vec::new();
		collect_rust_files(dir.as_ref(), &mut dir_files)?;
		dir_files.sort();
		files.extend(dir_files);
	}
	if files.is_empty() {
		return Ok(());
	}
	write_pot(&files, po_dir, package_name, package_version)?;
	Ok(())
}

/// Regenerates `<po_dir>/<package_name>.pot` from every package with
/// `[package.metadata.patois] translatable = true`, including dependencies.
///
/// `package_name` is the primary package, which names the file and fills in its header. The
/// function needs `xgettext` and `cargo` on `PATH`.
///
/// # Errors
///
/// Returns an error if `xgettext` or `cargo metadata` is missing or fails, if no tagged package has
/// any source files, or if a file can't be read or written.
pub fn gen_pot(
	project_root: impl AsRef<Path>,
	po_dir: impl AsRef<Path>,
	package_name: &str,
) -> Result<(), Box<dyn error::Error>> {
	let root = project_root.as_ref();
	let po_dir = po_dir.as_ref();
	fs::create_dir_all(po_dir)?;
	if Command::new("xgettext").arg("--version").output().is_err() {
		return Err("xgettext not found; install gettext tools".into());
	}
	let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
	let meta_output = Command::new(&cargo).args(["metadata", "--format-version", "1"]).current_dir(root).output()?;
	if !meta_output.status.success() {
		return Err("cargo metadata failed".into());
	}
	let meta: serde_json::Value = serde_json::from_slice(&meta_output.stdout)?;
	let mut packages: Vec<&serde_json::Value> =
		meta["packages"].as_array().ok_or("cargo metadata: missing packages")?.iter().collect();
	// Sort by name, not path: a package can be in a checkout for one contributor and in a registry
	// cache for another, and those sort differently. A stable order keeps the pot from changing
	// between machines.
	packages.sort_by_key(|pkg| pkg["name"].as_str().unwrap_or_default().to_string());
	let mut files: Vec<PathBuf> = Vec::new();
	for pkg in &packages {
		if pkg["metadata"]["patois"]["translatable"] != true {
			continue;
		}
		let manifest = pkg["manifest_path"].as_str().ok_or("cargo metadata: missing manifest_path")?;
		let src = Path::new(manifest).parent().ok_or("cargo metadata: manifest_path has no parent")?.join("src");
		let mut pkg_files: Vec<PathBuf> = Vec::new();
		collect_rust_files(&src, &mut pkg_files)?;
		pkg_files.sort();
		files.extend(pkg_files);
	}
	if files.is_empty() {
		return Err("no translatable source files found — check [package.metadata.patois] translatable = true".into());
	}
	let version = packages
		.iter()
		.find(|p| p["name"] == package_name)
		.and_then(|p| p["version"].as_str())
		.unwrap_or("0.0.0")
		.to_string();
	let output_file = po_dir.join(format!("{package_name}.pot"));
	if write_pot(&files, po_dir, package_name, &version)? {
		println!("Updated {}", output_file.display());
	} else {
		println!("No changes ({})", output_file.display());
	}
	Ok(())
}

/// Runs `xgettext` over `files`, merges the result into the pot, and returns whether the pot
/// changed.
fn write_pot(
	files: &[PathBuf],
	po_dir: &Path,
	package_name: &str,
	package_version: &str,
) -> Result<bool, Box<dyn error::Error>> {
	fs::create_dir_all(po_dir)?;
	let output_file = po_dir.join(format!("{package_name}.pot"));
	let temp_file = po_dir.join(format!("{package_name}.pot.new"));
	let scratch = env::temp_dir().join(format!("patois-build-pot-{}", process::id()));
	let sanitized = sanitize_files_into(&scratch, files);
	let result = sanitized.and_then(|sources| run_xgettext(&sources, &temp_file, package_name, package_version));
	let _ = fs::remove_dir_all(&scratch);
	result?;
	strip_format_flags(&temp_file)?;
	fix_charset_header(&temp_file)?;
	preserve_foreign_entries(&output_file, &temp_file)?;
	if pot_changed(&output_file, &temp_file) {
		fs::rename(&temp_file, &output_file)?;
		Ok(true)
	} else {
		fs::remove_file(&temp_file)?;
		Ok(false)
	}
}

/// Writes a sanitized copy of each file to `scratch`.
///
/// Each copy's name starts with its position in `files`, because two crates can each have a
/// `main.rs`. The names never reach the pot, because of `--no-location`.
fn sanitize_files_into(scratch: &Path, files: &[PathBuf]) -> Result<Vec<PathBuf>, Box<dyn error::Error>> {
	fs::create_dir_all(scratch)?;
	let mut sources = Vec::with_capacity(files.len());
	for (index, file) in files.iter().enumerate() {
		let name = file.file_name().map_or_else(|| "src.rs".to_string(), |n| n.to_string_lossy().into_owned());
		let dest = scratch.join(format!("{index:05}-{name}"));
		fs::write(&dest, sanitize_rust::sanitize_for_xgettext(&fs::read_to_string(file)?))?;
		sources.push(dest);
	}
	Ok(sources)
}

fn run_xgettext(
	sources: &[PathBuf],
	output: &Path,
	package_name: &str,
	package_version: &str,
) -> Result<(), Box<dyn error::Error>> {
	let mut cmd = Command::new("xgettext");
	cmd.arg("--keyword=t")
		.arg("--keyword=nt:1,2")
		.arg("--language=C")
		.arg("--from-code=UTF-8")
		.arg("--add-comments=TRANSLATORS")
		.arg("--no-location")
		.arg("--no-wrap")
		.arg(format!("--package-name={package_name}"))
		.arg(format!("--package-version={package_version}"))
		.arg(format!("--output={}", output.display()));
	for source in sources {
		cmd.arg(source);
	}
	if !cmd.status()?.success() {
		return Err("xgettext failed".into());
	}
	Ok(())
}

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn error::Error>> {
	if !dir.is_dir() {
		return Ok(());
	}
	for entry in fs::read_dir(dir)? {
		let path = entry?.path();
		if path.is_dir() {
			collect_rust_files(&path, files)?;
		} else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
			files.push(path);
		}
	}
	Ok(())
}

/// Removes the `c-format` and `no-c-format` flags. `xgettext` guesses them, and the same source
/// gets different flags from different versions, which churns the pot.
fn strip_format_flags(path: &Path) -> Result<(), Box<dyn error::Error>> {
	let content = fs::read_to_string(path)?;
	let mut out = String::with_capacity(content.len());
	for line in content.lines() {
		if let Some(rest) = line.strip_prefix("#,") {
			let kept: Vec<&str> =
				rest.split(',').map(str::trim).filter(|flag| *flag != "c-format" && *flag != "no-c-format").collect();
			if kept.is_empty() {
				continue;
			}
			out.push_str("#, ");
			out.push_str(&kept.join(", "));
			out.push('\n');
			continue;
		}
		out.push_str(line);
		out.push('\n');
	}
	fs::write(path, out)?;
	Ok(())
}

/// Replaces `xgettext`'s `CHARSET` placeholder with UTF-8. `msgmerge` refuses to read a file with
/// the placeholder if any string isn't pure ASCII.
fn fix_charset_header(path: &Path) -> Result<(), Box<dyn error::Error>> {
	let content = fs::read_to_string(path)?;
	let fixed = content.replacen("charset=CHARSET", "charset=UTF-8", 1);
	if fixed != content {
		fs::write(path, fixed)?;
	}
	Ok(())
}

fn pot_changed(old: &Path, new: &Path) -> bool {
	let strip_date = |s: &str| -> String {
		s.lines().filter(|l| !l.starts_with("\"POT-Creation-Date:")).collect::<Vec<_>>().join("\n")
	};
	let old = fs::read_to_string(old).unwrap_or_default();
	let Ok(new) = fs::read_to_string(new) else {
		return true;
	};
	strip_date(&old) != strip_date(&new)
}

/// Copies the entries that `old` got from a non-Rust scan to the end of `new`.
///
/// Without this function, a regenerated pot would lack those entries and always look changed.
/// The `#:` reference identifies them. Asking which strings the Rust scan didn't produce would also
/// catch a renamed Rust string, and keep it in the pot for good.
fn preserve_foreign_entries(old: &Path, new: &Path) -> Result<(), Box<dyn error::Error>> {
	let Ok(old_content) = fs::read_to_string(old) else {
		return Ok(());
	};
	let new_content = fs::read_to_string(new)?;
	let new_ids = collect_pot_msgids(&new_content);
	let mut additions = String::new();
	let mut seen: HashSet<String> = HashSet::new();
	for entry in collect_pot_entries_ordered(&old_content) {
		if entry.reference.is_some() && !new_ids.contains(&entry.msgid) && seen.insert(entry.msgid.clone()) {
			additions.push_str(&pot_entry_block(&entry));
		}
	}
	if !additions.is_empty() {
		let mut content = new_content;
		content.push_str(&additions);
		fs::write(new, content)?;
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn strip_format_flags_drops_c_format_lines_but_keeps_other_flags() {
		let content = "\
#. a comment
#, c-format
msgid \"Page %d\"
msgstr \"\"

#, fuzzy, c-format
msgid \"Old %s\"
msgstr \"\"

#, fuzzy
msgid \"Cancel\"
msgstr \"\"

msgid \"No flags here\"
msgstr \"\"
";
		let path = env::temp_dir().join(format!("patois-build-strip-format-flags-test-{}.pot", std::process::id()));
		fs::write(&path, content).unwrap();
		strip_format_flags(&path).unwrap();
		let result = fs::read_to_string(&path).unwrap();
		fs::remove_file(&path).unwrap();
		assert!(!result.contains("c-format"));
		assert!(result.contains("#, fuzzy\nmsgid \"Old %s\""));
		assert!(result.contains("#, fuzzy\nmsgid \"Cancel\""));
		assert!(result.contains("msgid \"No flags here\""));
	}

	#[test]
	fn fix_charset_header_replaces_the_placeholder() {
		let content = "\
msgid \"\"
msgstr \"\"
\"Content-Type: text/plain; charset=CHARSET\\n\"

msgid \"Cancel\"
msgstr \"\"
";
		let path = env::temp_dir().join(format!("patois-build-fix-charset-test-{}.pot", std::process::id()));
		fs::write(&path, content).unwrap();
		fix_charset_header(&path).unwrap();
		let result = fs::read_to_string(&path).unwrap();
		fs::remove_file(&path).unwrap();
		assert!(result.contains("charset=UTF-8"));
		assert!(!result.contains("charset=CHARSET"));
	}

	#[test]
	fn fix_charset_header_is_a_no_op_once_already_utf8() {
		let content = "\"Content-Type: text/plain; charset=UTF-8\\n\"\n";
		let path = env::temp_dir().join(format!("patois-build-fix-charset-noop-test-{}.pot", std::process::id()));
		fs::write(&path, content).unwrap();
		let before = fs::metadata(&path).unwrap().modified().unwrap();
		std::thread::sleep(std::time::Duration::from_millis(10));
		fix_charset_header(&path).unwrap();
		let after = fs::metadata(&path).unwrap().modified().unwrap();
		fs::remove_file(&path).unwrap();
		assert_eq!(before, after, "already-UTF-8 header should not be rewritten");
	}

	/// A pot with a header entry followed by `body`.
	fn pot(body: &str) -> String {
		format!("msgid \"\"\nmsgstr \"\"\n{body}")
	}

	fn scratch(name: &str) -> PathBuf {
		let dir = env::temp_dir().join(format!("patois-build-{name}-{}", std::process::id()));
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	#[test]
	fn preserve_foreign_entries_keeps_plural_entries_intact() {
		let dir = scratch("preserve-plural-test");
		let old = dir.join("old.pot");
		let new = dir.join("new.pot");
		// A plural entry from a Swift scan, which the Rust-only regeneration (simulated by
		// `new`) naturally doesn't produce. Preserving it has to keep both halves of the
		// plural and both msgstr slots, not flatten it to a singular entry.
		fs::write(
			&old,
			pot("\n#: swift\nmsgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n"),
		)
		.unwrap();
		fs::write(&new, pot("")).unwrap();
		preserve_foreign_entries(&old, &new).unwrap();
		let result = fs::read_to_string(&new).unwrap();
		fs::remove_dir_all(&dir).unwrap();
		assert!(result.contains("msgid \"{} result\"\nmsgid_plural \"{} results\""));
		assert!(result.contains("msgstr[0] \"\""));
		assert!(result.contains("msgstr[1] \"\""));
	}

	// A renamed string looks like a Swift or Kotlin one if the only question is whether the Rust
	// scan produced it, and carrying it forward would keep it in the pot for good.
	#[test]
	fn a_renamed_rust_msgid_is_not_carried_forward() {
		let dir = scratch("preserve-rename-test");
		let old = dir.join("old.pot");
		let new = dir.join("new.pot");
		fs::write(&old, pot("\nmsgid \"Paperback - {}\"\nmsgstr \"\"\n")).unwrap();
		fs::write(&new, pot("\nmsgid \"{} - Paperback\"\nmsgstr \"\"\n")).unwrap();
		preserve_foreign_entries(&old, &new).unwrap();
		let result = fs::read_to_string(&new).unwrap();
		fs::remove_dir_all(&dir).unwrap();
		assert!(result.contains("msgid \"{} - Paperback\""), "the new name should still be there");
		assert!(!result.contains("msgid \"Paperback - {}\""), "the renamed-away msgid should not be carried forward");
	}

	// A foreign entry has to come back out of `preserve_foreign_entries` still marked, or the
	// next run wouldn't recognize it, would drop it, and the Swift scan would append it again:
	// a rewritten pot (and a fresh POT-Creation-Date) on every other run, forever.
	#[test]
	fn a_preserved_foreign_entry_keeps_its_marker() {
		let dir = scratch("preserve-marker-test");
		let old = dir.join("old.pot");
		let new = dir.join("new.pot");
		fs::write(&old, pot("\n#: swift\nmsgid \"Continue\"\nmsgstr \"\"\n")).unwrap();
		fs::write(&new, pot("")).unwrap();
		preserve_foreign_entries(&old, &new).unwrap();
		let once = fs::read_to_string(&new).unwrap();
		assert!(once.contains("#: swift\nmsgid \"Continue\""), "marker lost on the way through");
		// Feed the result back in as the old pot: a second round has to be a no-op.
		fs::write(&old, &once).unwrap();
		fs::write(&new, pot("")).unwrap();
		preserve_foreign_entries(&old, &new).unwrap();
		let twice = fs::read_to_string(&new).unwrap();
		fs::remove_dir_all(&dir).unwrap();
		assert_eq!(once, twice, "preserving is not idempotent, so the pot would churn every run");
	}
}
