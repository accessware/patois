use std::{
	borrow::Cow,
	cmp::Ordering,
	io::{self, Read},
	str,
};

use crate::{EmbeddedLocale, plural::PluralRule};

const MAGIC_LE: u32 = 0x9504_12DE;
const MAGIC_BE: u32 = 0xDE12_0495;

/// The translations of one locale, and the rule that picks a plural form.
pub struct Catalog {
	source: Source,
	plural_rule: Option<PluralRule>,
}

enum Source {
	/// Embedded by `embed_domain!` as Rust statics, so nothing is parsed or checked at runtime.
	Baked { msgids: &'static [&'static str], locale: &'static EmbeddedLocale },
	/// A `.mo` file from disk.
	Mo(MoFile),
}

impl Catalog {
	pub fn parse(mut reader: impl Read) -> io::Result<Self> {
		let mut data = Vec::new();
		reader.read_to_end(&mut data)?;
		Self::from_bytes(Cow::Owned(data))
	}

	pub fn from_bytes(data: Cow<'static, [u8]>) -> io::Result<Self> {
		let mo = MoFile::new(data).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid .mo file"))?;
		let plural_rule = mo.header().and_then(PluralRule::parse_from_header);
		Ok(Self { source: Source::Mo(mo), plural_rule })
	}

	/// `msgids` is the domain's sorted list, which every locale's `starts` follows.
	pub fn baked(msgids: &'static [&'static str], locale: &'static EmbeddedLocale) -> Self {
		Self { source: Source::Baked { msgids, locale }, plural_rule: PluralRule::parse_from_header(locale.header) }
	}

	pub fn gettext<'a>(&'a self, key: &'a str) -> &'a str {
		self.forms(key).and_then(|mut forms| forms.next()).unwrap_or(key)
	}

	pub fn ngettext<'a>(&'a self, singular: &'a str, plural: &'a str, n: u64) -> &'a str {
		let english = if n == 1 { singular } else { plural };
		let Some(forms) = self.forms(singular) else {
			return english;
		};
		let last = forms.clone().count().saturating_sub(1);
		let index = self.plural_rule.as_ref().map_or_else(|| usize::from(n != 1), |rule| rule.index_for(n));
		forms.clone().nth(index.min(last)).unwrap_or(english)
	}

	/// The translated forms of `key`, leaving out empty ones, or `None` when it has none.
	fn forms(&self, key: &str) -> Option<impl Iterator<Item = &str> + Clone> {
		if key.is_empty() {
			return None;
		}
		let translation = match &self.source {
			Source::Baked { msgids, locale } => {
				let i = msgids.binary_search(&key).ok()?;
				let (start, end) = (*locale.starts.get(i)? as usize, *locale.starts.get(i + 1)? as usize);
				locale.text.get(start..end)?
			}
			Source::Mo(mo) => text(mo.translation(mo.find(key)?)?),
		};
		let forms = translation.split('\0').filter(|form| !form.is_empty());
		forms.clone().next()?;
		Some(forms)
	}
}

/// A `.mo` file read where it lies: lookups binary-search its table of original strings and
/// return slices of it.
struct MoFile {
	data: Cow<'static, [u8]>,
	little_endian: bool,
	count: usize,
	originals: usize,
	translations: usize,
	/// The entries in order of their original strings, for a file whose table isn't sorted.
	/// `msgfmt` always sorts it, so this is normally `None`.
	order: Option<Box<[u32]>>,
}

impl MoFile {
	fn new(data: Cow<'static, [u8]>) -> Option<Self> {
		let magic = u32::from_le_bytes(data.get(0..4)?.try_into().ok()?);
		let little_endian = match magic {
			MAGIC_LE => true,
			MAGIC_BE => false,
			_ => return None,
		};
		let mut mo = Self { data, little_endian, count: 0, originals: 0, translations: 0, order: None };
		mo.count = mo.u32_at(8)? as usize;
		mo.originals = mo.u32_at(12)? as usize;
		mo.translations = mo.u32_at(16)? as usize;
		// Every entry is checked once here, so a lookup can't fail on a bad offset or bad UTF-8.
		for i in 0..mo.count {
			str::from_utf8(mo.original(i)?).ok()?;
			str::from_utf8(mo.translation(i)?).ok()?;
		}
		let sorted = (1..mo.count).all(|i| mo.original(i - 1) <= mo.original(i));
		if !sorted {
			let mut order: Vec<u32> = (0..u32::try_from(mo.count).ok()?).collect();
			order.sort_by(|&a, &b| mo.original(a as usize).cmp(&mo.original(b as usize)));
			mo.order = Some(order.into_boxed_slice());
		}
		Some(mo)
	}

	/// The header is the entry whose original is empty.
	fn header(&self) -> Option<&str> {
		self.find("").and_then(|i| self.translation(i)).map(text)
	}

	fn u32_at(&self, pos: usize) -> Option<u32> {
		let bytes: [u8; 4] = self.data.get(pos..pos.checked_add(4)?)?.try_into().ok()?;
		Some(if self.little_endian { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
	}

	fn string(&self, table: usize, i: usize) -> Option<&[u8]> {
		let entry = table.checked_add(i.checked_mul(8)?)?;
		let len = self.u32_at(entry)? as usize;
		let offset = self.u32_at(entry + 4)? as usize;
		self.data.get(offset..offset.checked_add(len)?)
	}

	fn original(&self, i: usize) -> Option<&[u8]> {
		self.string(self.originals, i)
	}

	fn translation(&self, i: usize) -> Option<&[u8]> {
		self.string(self.translations, i)
	}

	/// The entry whose singular is `key`. A plural entry's original is "singular\0plural", and
	/// sorting by the whole original sorts by the singular too, because no byte is below `\0`.
	fn find(&self, key: &str) -> Option<usize> {
		let entry = |position: usize| self.order.as_ref().map_or(position, |order| order[position] as usize);
		let (mut low, mut high) = (0, self.count);
		while low < high {
			let middle = low + (high - low) / 2;
			let original = self.original(entry(middle))?;
			let singular = original.split(|&b| b == 0).next().unwrap_or(original);
			match singular.cmp(key.as_bytes()) {
				Ordering::Less => low = middle + 1,
				Ordering::Greater => high = middle,
				Ordering::Equal => return Some(entry(middle)),
			}
		}
		None
	}
}

// `MoFile::new` checked every string, so this never falls back.
fn text(bytes: &[u8]) -> &str {
	str::from_utf8(bytes).unwrap_or_default()
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
	fn an_unsorted_catalog_is_still_searched() {
		let mo = make_mo(&[("Zebra", "Zèbre"), ("Apple", "Pomme"), ("Mango", "Mangue")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert!(matches!(&cat.source, Source::Mo(mo) if mo.order.is_some()));
		assert_eq!(cat.gettext("Apple"), "Pomme");
		assert_eq!(cat.gettext("Mango"), "Mangue");
		assert_eq!(cat.gettext("Zebra"), "Zèbre");
		assert_eq!(cat.gettext("Kiwi"), "Kiwi");
	}

	#[test]
	fn a_borrowed_mo_file_is_read_where_it_lies() {
		let mo: &'static [u8] = Box::leak(make_mo(&[("Hello", "Hallo")]).into_boxed_slice());
		let cat = Catalog::from_bytes(Cow::Borrowed(mo)).unwrap();
		assert!(matches!(&cat.source, Source::Mo(mo) if mo.order.is_none()));
		assert_eq!(cat.gettext("Hello").as_ptr(), mo[mo.len() - "Hallo".len() - 1..].as_ptr());
	}

	/// Laid out as `embed_domain!` generates it: "Open" has no translation in this locale.
	static MSGIDS: [&str; 4] = ["Close", "Open", "Save", "file"];
	static GERMAN: EmbeddedLocale = EmbeddedLocale {
		code: "de",
		header: "Plural-Forms: nplurals=2; plural=(n != 1);\n",
		text: "SchließenSpeichernDatei\0Dateien",
		// "Schließen" is 10 bytes, since ß takes two.
		starts: &[0, 10, 10, 19, 32],
	};

	#[test]
	fn a_baked_catalog_finds_each_translation_by_its_place_in_the_shared_msgids() {
		let cat = Catalog::baked(&MSGIDS, &GERMAN);
		assert_eq!(cat.gettext("Close"), "Schließen");
		assert_eq!(cat.gettext("Save"), "Speichern");
		assert_eq!(cat.gettext("Open"), "Open", "a msgid only another locale translates");
		assert_eq!(cat.gettext("Print"), "Print", "a msgid no locale has");
		assert_eq!(cat.ngettext("file", "files", 1), "Datei");
		assert_eq!(cat.ngettext("file", "files", 4), "Dateien");
		assert_eq!(cat.gettext(""), "");
	}

	#[test]
	fn an_empty_translation_leaves_the_english() {
		let mo = make_mo(&[("Hello", ""), ("item\0items", "\0")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.gettext("Hello"), "Hello");
		assert_eq!(cat.ngettext("item", "items", 2), "items");
	}

	#[test]
	fn a_plural_entry_is_found_by_its_singular_beside_a_longer_one() {
		let mo = make_mo(&[("file\0files", "fichier\0fichiers"), ("file name", "nom de fichier")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.ngettext("file", "files", 3), "fichiers");
		assert_eq!(cat.gettext("file name"), "nom de fichier");
	}

	#[test]
	fn a_truncated_catalog_is_rejected() {
		let mut mo = make_mo(&[("Hello", "Hallo")]);
		mo.truncate(mo.len() - 3);
		assert!(Catalog::parse(Cursor::new(mo)).is_err());
		assert!(Catalog::parse(Cursor::new(vec![0xDE, 0x12])).is_err());
	}

	#[test]
	fn missing_header_falls_back_to_english_style_rule() {
		let mo = make_mo(&[("apple\0apples", "one\0many")]);
		let cat = Catalog::parse(Cursor::new(mo)).unwrap();
		assert_eq!(cat.ngettext("apple", "apples", 1), "one");
		assert_eq!(cat.ngettext("apple", "apples", 5), "many");
	}
}
