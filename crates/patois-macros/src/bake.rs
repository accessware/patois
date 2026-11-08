//! Turns compiled `.mo` catalogs into the tables `embed_domain!` writes out as Rust statics.

use std::collections::{BTreeSet, HashMap};

/// What `embed_domain!` generates for one crate.
#[derive(Debug, PartialEq, Eq)]
pub struct Baked {
	/// Every msgid any locale has, sorted by bytes, which is how the runtime searches it.
	pub msgids: Vec<String>,
	pub locales: Vec<BakedLocale>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct BakedLocale {
	pub code: String,
	pub header: String,
	/// The translations in the order of `msgids`, one after another.
	pub text: String,
	/// Where each msgid's translation starts in `text`, and one more for the end of the last.
	pub starts: Vec<u32>,
}

/// One `.mo` file's entries: its header, and each singular msgid with its translation.
struct MoEntries {
	header: String,
	translations: HashMap<String, String>,
}

/// `catalogs` holds each locale's code and `.mo` bytes. A file that isn't a valid `.mo` gives the
/// error that names it.
pub fn bake(catalogs: &[(String, Vec<u8>)]) -> Result<Baked, String> {
	let mut read = Vec::new();
	for (code, bytes) in catalogs {
		read.push((code, read_mo(bytes).ok_or_else(|| format!("the {code} catalog is not a valid .mo file"))?));
	}
	let msgids: BTreeSet<&str> = read.iter().flat_map(|(_, mo)| mo.translations.keys().map(String::as_str)).collect();
	let locales = read
		.iter()
		.map(|(code, mo)| {
			let mut text = String::new();
			let mut starts = Vec::with_capacity(msgids.len() + 1);
			for msgid in &msgids {
				starts.push(offset(&text)?);
				text.push_str(mo.translations.get(*msgid).map_or("", String::as_str));
			}
			starts.push(offset(&text)?);
			Ok(BakedLocale { code: (*code).clone(), header: mo.header.clone(), text, starts })
		})
		.collect::<Result<_, String>>()?;
	Ok(Baked { msgids: msgids.into_iter().map(String::from).collect(), locales })
}

fn offset(text: &str) -> Result<u32, String> {
	u32::try_from(text.len()).map_err(|_| "a catalog holds more than 4 GiB of translations".to_string())
}

/// Reads a `.mo` file. A plural entry's original is "singular\0plural"; it is kept by its singular,
/// which is how the runtime looks it up. An entry with no translation is left out.
fn read_mo(data: &[u8]) -> Option<MoEntries> {
	let little_endian = match data.get(0..4)? {
		[0xDE, 0x12, 0x04, 0x95] => true,
		[0x95, 0x04, 0x12, 0xDE] => false,
		_ => return None,
	};
	let u32_at = |pos: usize| -> Option<usize> {
		let bytes: [u8; 4] = data.get(pos..pos.checked_add(4)?)?.try_into().ok()?;
		usize::try_from(if little_endian { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) }).ok()
	};
	let string = |table: usize, i: usize| -> Option<&str> {
		let entry = table.checked_add(i.checked_mul(8)?)?;
		let (len, offset) = (u32_at(entry)?, u32_at(entry + 4)?);
		std::str::from_utf8(data.get(offset..offset.checked_add(len)?)?).ok()
	};
	let (count, originals, translations) = (u32_at(8)?, u32_at(12)?, u32_at(16)?);
	let mut entries = MoEntries { header: String::new(), translations: HashMap::new() };
	for i in 0..count {
		let original = string(originals, i)?;
		let translation = string(translations, i)?;
		let singular = original.split('\0').next().unwrap_or(original);
		if singular.is_empty() {
			translation.clone_into(&mut entries.header);
		} else if translation.split('\0').any(|form| !form.is_empty()) {
			entries.translations.insert(singular.to_string(), translation.to_string());
		}
	}
	Some(entries)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A little-endian `.mo` file with `entries` in the order given.
	fn mo(entries: &[(&str, &str)]) -> Vec<u8> {
		let count = u32::try_from(entries.len()).unwrap();
		let mut strings = Vec::new();
		let mut tables = [Vec::new(), Vec::new()];
		let start = 28 + count * 16;
		for (table, pick) in tables.iter_mut().zip([0, 1]) {
			for entry in entries {
				let s = if pick == 0 { entry.0 } else { entry.1 };
				table.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
				table.extend_from_slice(&(start + u32::try_from(strings.len()).unwrap()).to_le_bytes());
				strings.extend_from_slice(s.as_bytes());
				strings.push(0);
			}
		}
		let mut out = vec![0xDE, 0x12, 0x04, 0x95, 0, 0, 0, 0];
		for value in [count, 28, 28 + count * 8, 0, 0] {
			out.extend_from_slice(&value.to_le_bytes());
		}
		out.extend(tables.concat());
		out.extend(strings);
		out
	}

	#[test]
	fn every_locale_follows_one_sorted_list_of_msgids() {
		let german =
			mo(&[("", "Plural-Forms: nplurals=2; plural=(n != 1);\n"), ("Save", "Speichern"), ("Close", "Schließen")]);
		let french = mo(&[("Open", "Ouvrir"), ("Close", "Fermer")]);
		let baked = bake(&[("de".to_string(), german), ("fr".to_string(), french)]).unwrap();
		assert_eq!(baked.msgids, ["Close", "Open", "Save"]);
		let de = &baked.locales[0];
		assert_eq!(de.header, "Plural-Forms: nplurals=2; plural=(n != 1);\n");
		assert_eq!(de.text, "SchließenSpeichern");
		assert_eq!(de.starts, [0, 10, 10, 19], "Open has no German translation, so it takes no room");
		let fr = &baked.locales[1];
		assert_eq!(fr.text, "FermerOuvrir");
		assert_eq!(fr.starts, [0, 6, 12, 12]);
	}

	#[test]
	fn a_plural_entry_is_kept_by_its_singular_with_its_forms() {
		let baked = bake(&[("de".to_string(), mo(&[("file\0files", "Datei\0Dateien")]))]).unwrap();
		assert_eq!(baked.msgids, ["file"]);
		assert_eq!(baked.locales[0].text, "Datei\0Dateien");
	}

	#[test]
	fn an_untranslated_entry_is_left_out() {
		let baked =
			bake(&[("de".to_string(), mo(&[("Save", ""), ("Close", "Schließen"), ("item\0items", "\0")]))]).unwrap();
		assert_eq!(baked.msgids, ["Close"]);
	}

	#[test]
	fn a_file_that_is_not_a_catalog_is_named() {
		let error = bake(&[("de".to_string(), b"not a catalog".to_vec())]).unwrap_err();
		assert!(error.contains("de"), "{error}");
	}
}
