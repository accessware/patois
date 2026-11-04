use std::{error, fmt::Write as _, fs, path::Path};

use crate::po;

fn parse_po_entries(content: &str) -> Vec<(String, String)> {
	po::PoDocument::parse(content)
		.entries
		.into_iter()
		.filter(|e| !e.msgstr.is_empty())
		.map(|e| (e.msgid, e.msgstr))
		.collect()
}

fn escape_for_localizable_strings(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			c => out.push(c),
		}
	}
	out
}

/// Writes the translated strings of each `<lang>.po` in `po_dir` to
/// `<ios_dir>/<lang>.lproj/Localizable.strings`.
///
/// # Errors
///
/// Returns an error if a file can't be read or written.
pub fn gen_ios_strings(po_dir: impl AsRef<Path>, ios_dir: impl AsRef<Path>) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	let ios_dir = ios_dir.as_ref();
	let dir_entries = fs::read_dir(po_dir).map_err(|e| format!("cannot read {}: {e}", po_dir.display()))?;
	for entry in dir_entries {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		let content = fs::read_to_string(&path)?;
		let translations = parse_po_entries(&content);
		if translations.is_empty() {
			continue;
		}
		let lproj = ios_dir.join(format!("{lang}.lproj"));
		fs::create_dir_all(&lproj)?;
		let out_path = lproj.join("Localizable.strings");
		let mut out = String::new();
		for (msgid, msgstr) in &translations {
			let _ = writeln!(
				out,
				"\"{}\" = \"{}\";",
				escape_for_localizable_strings(msgid),
				escape_for_localizable_strings(msgstr)
			);
		}
		let existing = fs::read_to_string(&out_path).unwrap_or_default();
		if existing != out {
			fs::write(&out_path, &out)?;
			println!("Updated {}", out_path.display());
		}
	}
	Ok(())
}

/// Writes the translated strings of each `<lang>.po` in `po_dir` to
/// `<assets_dir>/translations/<lang>.json`, as an object that maps each msgid to its translation.
///
/// # Errors
///
/// Returns an error if a file can't be read or written.
pub fn gen_android_strings(
	po_dir: impl AsRef<Path>,
	assets_dir: impl AsRef<Path>,
) -> Result<(), Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	let translations_dir = assets_dir.as_ref().join("translations");
	let dir_entries = fs::read_dir(po_dir).map_err(|e| format!("cannot read {}: {e}", po_dir.display()))?;
	for entry in dir_entries {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		let lang = match path.file_stem().and_then(|s| s.to_str()) {
			Some(l) => l.to_string(),
			None => continue,
		};
		let content = fs::read_to_string(&path)?;
		let translations = parse_po_entries(&content);
		if translations.is_empty() {
			continue;
		}
		fs::create_dir_all(&translations_dir)?;
		let out_path = translations_dir.join(format!("{lang}.json"));
		let map: serde_json::Map<String, serde_json::Value> =
			translations.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect();
		let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
		let json = json + "\n";
		let existing = fs::read_to_string(&out_path).unwrap_or_default();
		if existing != json {
			fs::write(&out_path, &json)?;
			println!("Updated {}", out_path.display());
		}
	}
	Ok(())
}
