//! An Inno Setup installer's translations, from the same catalogs as the app's.

use std::{collections::BTreeMap, error, fmt::Write as _, fs, path::Path};

use crate::po::PoDocument;

/// Each catalog's code, lower case with `_`, and the Inno Setup messages file for it. The ones
/// Inno Setup ships only as unofficial translations are bundled here, since they aren't in its
/// `Languages` folder.
const LANGUAGES: &[(&str, Isl)] = &[
	("ar", Isl::Shipped("Arabic")),
	("bg", Isl::Shipped("Bulgarian")),
	("bs", Isl::Bundled("Bosnian", include_bytes!("inno/Bosnian.isl"))),
	("ca", Isl::Shipped("Catalan")),
	("co", Isl::Shipped("Corsican")),
	("cs", Isl::Shipped("Czech")),
	("da", Isl::Shipped("Danish")),
	("de", Isl::Shipped("German")),
	("es", Isl::Shipped("Spanish")),
	("fi", Isl::Shipped("Finnish")),
	("fr", Isl::Shipped("French")),
	("he", Isl::Shipped("Hebrew")),
	("hu", Isl::Shipped("Hungarian")),
	("hy", Isl::Shipped("Armenian")),
	("it", Isl::Shipped("Italian")),
	("ja", Isl::Shipped("Japanese")),
	("ko", Isl::Shipped("Korean")),
	("nb", Isl::Shipped("Norwegian")),
	("nl", Isl::Shipped("Dutch")),
	("no", Isl::Shipped("Norwegian")),
	("pl", Isl::Shipped("Polish")),
	("pt", Isl::Shipped("Portuguese")),
	("pt_br", Isl::Shipped("BrazilianPortuguese")),
	("ru", Isl::Shipped("Russian")),
	("sk", Isl::Shipped("Slovak")),
	("sl", Isl::Shipped("Slovenian")),
	("sr", Isl::Bundled("SerbianLatin", include_bytes!("inno/SerbianLatin.isl"))),
	("sr_latn", Isl::Bundled("SerbianLatin", include_bytes!("inno/SerbianLatin.isl"))),
	("sv", Isl::Shipped("Swedish")),
	("ta", Isl::Shipped("Tamil")),
	("tr", Isl::Shipped("Turkish")),
	("uk", Isl::Shipped("Ukrainian")),
	("vi", Isl::Bundled("Vietnamese", include_bytes!("inno/Vietnamese.isl"))),
	("zh_cn", Isl::Bundled("ChineseSimplified", include_bytes!("inno/ChineseSimplified.isl"))),
];

#[derive(Clone, Copy)]
enum Isl {
	/// In Inno Setup's own `Languages` folder.
	Shipped(&'static str),
	/// Written out to the folder the caller gives.
	Bundled(&'static str, &'static [u8]),
}

/// The two sections [`gen_inno_sections`] writes, to be put into the installer script.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InnoSections {
	/// The lines of `[Languages]`, English first.
	pub languages: String,
	/// The lines of `[CustomMessages]`: each message in English for every language, then each
	/// catalog's translations.
	pub custom_messages: String,
	/// The catalogs Inno Setup has no messages file for, which the installer leaves out.
	pub skipped: Vec<String>,
}

/// Writes the `[Languages]` and `[CustomMessages]` lines of an Inno Setup script for every
/// `<lang>.po` in `po_dir`.
///
/// `messages` are the installer's own strings, each a message name and its English, marked for
/// translation in the app's source so they reach the catalogs. The script uses them as
/// `{cm:Name}`. Machine translations, which are fuzzy, are used, as the app uses them. A string a
/// catalog lacks falls back to English.
///
/// The messages files Inno Setup ships only as unofficial translations are written to
/// `languages_dir`, which the script names by path. The script must start with a UTF-8 byte order
/// mark, or Inno Setup reads it in the system's code page.
///
/// # Errors
///
/// Returns an error if a catalog can't be read or a messages file can't be written.
pub fn gen_inno_sections(
	po_dir: impl AsRef<Path>,
	messages: &[(&str, &str)],
	languages_dir: impl AsRef<Path>,
) -> Result<InnoSections, Box<dyn error::Error>> {
	let po_dir = po_dir.as_ref();
	let languages_dir = languages_dir.as_ref();
	let mut catalogs = BTreeMap::new();
	for entry in fs::read_dir(po_dir).map_err(|e| format!("cannot read {}: {e}", po_dir.display()))? {
		let path = entry?.path();
		if path.extension().and_then(|e| e.to_str()) != Some("po") {
			continue;
		}
		if let Some(code) = path.file_stem().and_then(|s| s.to_str()) {
			catalogs.insert(code.to_string(), path);
		}
	}
	let mut sections = InnoSections {
		languages: "\tName: \"en\"; MessagesFile: \"compiler:Default.isl\"\n".to_string(),
		..InnoSections::default()
	};
	for (name, english) in messages {
		let _ = writeln!(sections.custom_messages, "{name}={}", one_line(english));
	}
	for (code, path) in catalogs {
		let Some(isl) = isl_for(&code) else {
			sections.skipped.push(code);
			continue;
		};
		let file = match isl {
			Isl::Shipped(name) => format!("compiler:Languages\\{name}.isl"),
			Isl::Bundled(name, bytes) => {
				fs::create_dir_all(languages_dir)?;
				let out = languages_dir.join(format!("{name}.isl"));
				if fs::read(&out).ok().as_deref() != Some(bytes) {
					fs::write(&out, bytes)?;
				}
				out.display().to_string()
			}
		};
		let name = code.replace('-', "_");
		let _ = writeln!(sections.languages, "\tName: \"{name}\"; MessagesFile: \"{file}\"");
		let translations = translations(&fs::read_to_string(&path)?);
		for (message, english) in messages {
			if let Some(translated) = translations.get(*english) {
				let _ = writeln!(sections.custom_messages, "{name}.{message}={}", one_line(translated));
			}
		}
	}
	// Each goes where a script's placeholder line was, which brings its own line end.
	for section in [&mut sections.languages, &mut sections.custom_messages] {
		section.truncate(section.trim_end_matches('\n').len());
	}
	Ok(sections)
}

fn isl_for(code: &str) -> Option<Isl> {
	let code = code.to_ascii_lowercase().replace('-', "_");
	LANGUAGES.iter().find(|(known, _)| *known == code).map(|(_, isl)| *isl)
}

/// A message is one line of the script.
fn one_line(text: &str) -> String {
	text.replace(['\r', '\n'], " ")
}

fn translations(catalog: &str) -> BTreeMap<String, String> {
	PoDocument::parse(catalog)
		.entries
		.into_iter()
		.filter(|entry| !entry.msgid.is_empty() && !entry.msgstr.is_empty())
		.map(|entry| (entry.msgid, entry.msgstr))
		.collect()
}

#[cfg(test)]
mod tests {
	use std::{env, fs, path::PathBuf, process};

	use super::gen_inno_sections;

	fn scratch(name: &str) -> PathBuf {
		let dir = env::temp_dir().join(format!("patois-inno-{name}-{}", process::id()));
		let _ = fs::remove_dir_all(&dir);
		fs::create_dir_all(&dir).unwrap();
		dir
	}

	const HEADER: &str = "msgid \"\"\nmsgstr \"Language: x\\n\"\n\n";

	#[test]
	fn each_catalog_gets_its_messages_file_and_its_translations() {
		let po = scratch("po");
		let out = scratch("languages");
		fs::write(po.join("de.po"), format!("{HEADER}msgid \"Create a shortcut\"\nmsgstr \"Verknüpfung erstellen\"\n\n#, fuzzy\nmsgid \"Two\\nlines\"\nmsgstr \"Zwei\\nZeilen\"\n")).unwrap();
		fs::write(po.join("zh_CN.po"), HEADER).unwrap();
		fs::write(po.join("xx.po"), HEADER).unwrap();
		let sections =
			gen_inno_sections(&po, &[("Shortcut", "Create a shortcut"), ("Lines", "Two\nlines")], &out).unwrap();
		let languages: Vec<&str> = sections.languages.lines().collect();
		assert_eq!(languages[0], "\tName: \"en\"; MessagesFile: \"compiler:Default.isl\"");
		assert_eq!(languages[1], "\tName: \"de\"; MessagesFile: \"compiler:Languages\\German.isl\"");
		assert!(languages[2].starts_with("\tName: \"zh_CN\"; MessagesFile: \""), "{}", languages[2]);
		assert!(out.join("ChineseSimplified.isl").is_file(), "the unofficial file is written for the script to name");
		assert_eq!(sections.skipped, ["xx"]);
		assert_eq!(
			sections.custom_messages.lines().collect::<Vec<_>>(),
			[
				"Shortcut=Create a shortcut",
				"Lines=Two lines",
				"de.Shortcut=Verknüpfung erstellen",
				"de.Lines=Zwei Zeilen"
			]
		);
	}

	#[test]
	fn a_catalog_code_is_matched_whatever_its_case_or_separator() {
		assert!(super::isl_for("pt-BR").is_some());
		assert!(super::isl_for("PT_br").is_some());
		assert!(super::isl_for("sr_Latn").is_some());
		assert!(super::isl_for("klingon").is_none());
	}
}
