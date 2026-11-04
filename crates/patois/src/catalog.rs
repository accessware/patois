use std::{
	collections::HashMap,
	io::{self, Read},
	str,
};

use crate::plural::PluralRule;

const MAGIC_LE: u32 = 0x9504_12DE;
const MAGIC_BE: u32 = 0xDE12_0495;

pub struct Catalog {
	translations: HashMap<String, Vec<String>>,
	plural_rule: Option<PluralRule>,
}

impl Catalog {
	pub fn parse(mut reader: impl Read) -> io::Result<Self> {
		let mut data = Vec::new();
		reader.read_to_end(&mut data)?;
		parse_mo(&data).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid .mo file"))
	}

	pub fn gettext<'a>(&'a self, key: &'a str) -> &'a str {
		self.translations.get(key).and_then(|forms| forms.first()).map_or(key, String::as_str)
	}

	pub fn ngettext<'a>(&'a self, singular: &'a str, plural: &'a str, n: u64) -> &'a str {
		let english = if n == 1 { singular } else { plural };
		let Some(forms) = self.translations.get(singular) else {
			return english;
		};
		let last = forms.len().saturating_sub(1);
		let index = self.plural_rule.as_ref().map_or_else(|| usize::from(n != 1), |rule| rule.index_for(n));
		forms.get(index.min(last)).map_or(english, String::as_str)
	}
}

fn parse_mo(data: &[u8]) -> Option<Catalog> {
	if data.len() < 28 {
		return None;
	}
	let magic = u32::from_le_bytes(data[0..4].try_into().ok()?);
	let le = match magic {
		MAGIC_LE => true,
		MAGIC_BE => false,
		_ => return None,
	};
	let u32_at = |pos: usize| -> Option<u32> {
		let bytes: [u8; 4] = data.get(pos..pos + 4)?.try_into().ok()?;
		Some(if le { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
	};
	let n = u32_at(8)? as usize;
	let orig_table = u32_at(12)? as usize;
	let trans_table = u32_at(16)? as usize;
	let mut translations: HashMap<String, Vec<String>> = HashMap::with_capacity(n);
	let mut plural_rule = None;
	for i in 0..n {
		let orig_len = u32_at(orig_table + i * 8)? as usize;
		let orig_off = u32_at(orig_table + i * 8 + 4)? as usize;
		let trans_len = u32_at(trans_table + i * 8)? as usize;
		let trans_off = u32_at(trans_table + i * 8 + 4)? as usize;
		let key_bytes = data.get(orig_off..orig_off + orig_len)?;
		let val_bytes = data.get(trans_off..trans_off + trans_len)?;
		let key = str::from_utf8(key_bytes).ok()?;
		let val = str::from_utf8(val_bytes).ok()?;
		// A plural entry's key is "singular\0plural" and its value is "form0\0form1\0...".
		let singular = key.split('\0').next()?;
		if singular.is_empty() {
			plural_rule = PluralRule::parse_from_header(val);
			continue;
		}
		if val.is_empty() {
			continue;
		}
		let forms: Vec<String> = val.split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
		if !forms.is_empty() {
			translations.insert(singular.to_string(), forms);
		}
	}
	Some(Catalog { translations, plural_rule })
}

#[cfg(test)]
pub mod tests {
	use std::io::Cursor;

	use super::*;

	#[allow(clippy::cast_possible_truncation, reason = "test catalogs are far smaller than 4 GiB")]
	pub fn make_mo(entries: &[(&str, &str)]) -> Vec<u8> {
		let n = entries.len();
		let orig_table_off: u32 = 28;
		let trans_table_off: u32 = orig_table_off + (n as u32) * 8;
		let strings_off: u32 = trans_table_off + (n as u32) * 8;
		let mut orig_strings: Vec<u8> = Vec::new();
		let mut trans_strings: Vec<u8> = Vec::new();
		let mut orig_table: Vec<u8> = Vec::new();
		let mut trans_table: Vec<u8> = Vec::new();
		let mut cur_orig = strings_off;
		for (key, _) in entries {
			orig_table.extend_from_slice(&(key.len() as u32).to_le_bytes());
			orig_table.extend_from_slice(&cur_orig.to_le_bytes());
			cur_orig += key.len() as u32 + 1;
			orig_strings.extend_from_slice(key.as_bytes());
			orig_strings.push(0);
		}
		let trans_base = cur_orig;
		let mut cur_trans2 = trans_base;
		for (_, val) in entries {
			trans_table.extend_from_slice(&(val.len() as u32).to_le_bytes());
			trans_table.extend_from_slice(&cur_trans2.to_le_bytes());
			cur_trans2 += val.len() as u32 + 1;
			trans_strings.extend_from_slice(val.as_bytes());
			trans_strings.push(0);
		}
		let mut mo: Vec<u8> = Vec::new();
		mo.extend_from_slice(&MAGIC_LE.to_le_bytes());
		let revision = 0u32;
		let hash_size = 0u32;
		let hash_offset = 0u32;
		mo.extend_from_slice(&revision.to_le_bytes());
		mo.extend_from_slice(&(n as u32).to_le_bytes());
		mo.extend_from_slice(&orig_table_off.to_le_bytes());
		mo.extend_from_slice(&trans_table_off.to_le_bytes());
		mo.extend_from_slice(&hash_size.to_le_bytes());
		mo.extend_from_slice(&hash_offset.to_le_bytes());
		mo.extend_from_slice(&orig_table);
		mo.extend_from_slice(&trans_table);
		mo.extend_from_slice(&orig_strings);
		mo.extend_from_slice(&trans_strings);
		mo
	}

	#[test]
	fn parses_simple_entries() {
		let mo = make_mo(&[("Hello", "Hallo"), ("World", "Welt")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.gettext("Hello"), "Hallo");
		assert_eq!(cat.gettext("World"), "Welt");
		assert_eq!(cat.gettext("Missing"), "Missing");
	}

	#[test]
	fn plural_fallback_with_no_translation() {
		let mo = make_mo(&[]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.ngettext("item", "items", 1), "item");
		assert_eq!(cat.ngettext("item", "items", 5), "items");
	}

	#[test]
	fn header_plural_forms_rule_picks_the_right_form() {
		let header = "Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2);\n";
		let mo = make_mo(&[("", header), ("apple\0apples", "one\0few\0many")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.ngettext("apple", "apples", 1), "one");
		assert_eq!(cat.ngettext("apple", "apples", 2), "few");
		assert_eq!(cat.ngettext("apple", "apples", 5), "many");
		assert_eq!(cat.ngettext("apple", "apples", 11), "many");
	}

	#[test]
	fn missing_header_falls_back_to_english_style_rule() {
		let mo = make_mo(&[("apple\0apples", "one\0many")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.ngettext("apple", "apples", 1), "one");
		assert_eq!(cat.ngettext("apple", "apples", 5), "many");
	}
}
