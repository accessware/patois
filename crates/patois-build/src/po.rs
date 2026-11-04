//! Edits `.po` files in place.
//!
//! Finds the entries that need a translation and writes results into only those entries, so every
//! other line of the file stays exactly as it was.

use std::ops::Range;

use crate::entries::{po_unescape, pot_escape};

/// A translation to write to one entry.
///
/// The variants are separate so that a caller can't write one string to a plural entry by accident,
/// which would collapse its `msgstr[N]` lines into one `msgstr`.
pub enum Translation {
	/// The translation of an ordinary entry.
	Singular(String),
	/// One string for each plural form, starting with `msgstr[0]`. [`PoDocument::nplurals`] gives
	/// the number of forms.
	Plural(Vec<String>),
}

/// One entry of a `.po` file.
pub struct PoEntryLoc {
	/// The source string. It's empty for the header entry.
	pub msgid: String,
	/// The translation of an ordinary entry. It's empty for a plural entry.
	pub msgstr: String,
	/// The plural source string of a plural entry, or `None` for an ordinary entry.
	pub msgid_plural: Option<String>,
	/// The `msgstr[N]` values of a plural entry, in order. It's empty for an ordinary entry.
	pub msgstr_plural: Vec<String>,
	/// The `#.` note above the entry, without the `TRANSLATORS:` tag. The lines of a longer note
	/// are joined by spaces.
	pub comment: Option<String>,
	/// Whether the entry has the `fuzzy` flag.
	pub is_fuzzy: bool,
	/// Whether the entry has a `#|` comment, which `msgmerge` writes when it matches an entry to a
	/// changed source string.
	///
	/// A fuzzy entry without one is a machine translation, as [`PoDocument::apply`] leaves it.
	/// [`PoDocument::needs_translation`] relies on the difference, because otherwise it would ask
	/// for a machine translation again on every run.
	pub has_prev_msgid: bool,
	/// The `#,` and `#|` lines above the msgid, or an empty range at the msgid if there are none.
	flag_block: Range<usize>,
	msgstr_range: Range<usize>,
}

/// An entry that `msgmerge` marked obsolete (`#~`) because its msgid left the template. It's still
/// the best translation of a string that comes back with the same words.
///
/// Plural entries and entries with a `msgctxt` aren't kept, because they aren't the same string as
/// an ordinary entry with the same words.
pub struct ObsoleteEntry {
	/// The source string.
	pub msgid: String,
	/// The translation.
	pub msgstr: String,
	/// Whether the entry has the `fuzzy` flag.
	pub is_fuzzy: bool,
}

/// A parsed `.po` file.
///
/// The document keeps the original lines, so [`Self::render`] reproduces every entry that wasn't
/// changed exactly.
pub struct PoDocument {
	lines: Vec<String>,
	/// The entries of the file, including the header entry.
	pub entries: Vec<PoEntryLoc>,
	/// The `#~` entries, which are never candidates for translation and never changed.
	pub obsolete: Vec<ObsoleteEntry>,
}

impl PoDocument {
	/// Parses the text of a `.po` file.
	#[must_use]
	pub fn parse(content: &str) -> Self {
		let lines: Vec<String> = content.lines().map(str::to_string).collect();
		let mut entries = Vec::new();
		let mut obsolete = Vec::new();
		let mut flag_line: Option<usize> = None;
		let mut prev_msgid_range: Option<(usize, usize)> = None;
		let mut pending_comment: Vec<String> = Vec::new();
		let mut i = 0;
		while i < lines.len() {
			let trimmed = lines[i].trim_start();
			if trimmed.starts_with("#~") {
				let is_fuzzy = flag_line.is_some_and(|fl| lines[fl].contains("fuzzy"));
				let mut block = Vec::new();
				while let Some(rest) = lines.get(i).and_then(|line| line.trim_start().strip_prefix("#~")) {
					block.push(rest.trim_start());
					i += 1;
				}
				obsolete.extend(parse_obsolete(&block, is_fuzzy));
				flag_line = None;
				prev_msgid_range = None;
				pending_comment.clear();
				continue;
			}
			if trimmed.starts_with('#') {
				if let Some(note) = trimmed.strip_prefix("#.") {
					let note = note.trim();
					pending_comment.push(note.strip_prefix("TRANSLATORS:").unwrap_or(note).trim().to_string());
				} else if trimmed.starts_with("#,") {
					flag_line = Some(i);
				} else if trimmed.starts_with("#|") {
					prev_msgid_range = Some(prev_msgid_range.map_or((i, i + 1), |(start, _)| (start, i + 1)));
				}
				i += 1;
				continue;
			}
			if trimmed.is_empty() {
				flag_line = None;
				prev_msgid_range = None;
				pending_comment.clear();
				i += 1;
				continue;
			}
			let Some(rest) = trimmed.strip_prefix("msgid ") else {
				i += 1;
				continue;
			};
			let msgid_start = i;
			let mut msgid = po_unescape(rest);
			i += 1;
			while i < lines.len() && lines[i].trim_start().starts_with('"') {
				msgid.push_str(&po_unescape(lines[i].trim_start()));
				i += 1;
			}
			let msgid_plural =
				lines.get(i).and_then(|line| line.trim_start().strip_prefix("msgid_plural ")).map(|rest| {
					let mut plural = po_unescape(rest);
					i += 1;
					while i < lines.len() && lines[i].trim_start().starts_with('"') {
						plural.push_str(&po_unescape(lines[i].trim_start()));
						i += 1;
					}
					plural
				});
			let parsed = if msgid_plural.is_some() {
				read_plural_msgstrs(&lines, &mut i).map(|(forms, range)| (String::new(), forms, range))
			} else {
				read_msgstr(&lines, &mut i).map(|(msgstr, range)| (msgstr, Vec::new(), range))
			};
			if let Some((msgstr, msgstr_plural, msgstr_range)) = parsed {
				let is_fuzzy = flag_line.is_some_and(|fl| lines[fl].contains("fuzzy"));
				let flag_block_start = match (flag_line, prev_msgid_range) {
					(Some(f), Some((p, _))) => f.min(p),
					(Some(f), None) => f,
					(None, Some((p, _))) => p,
					(None, None) => msgid_start,
				};
				entries.push(PoEntryLoc {
					msgid,
					msgstr,
					msgid_plural,
					msgstr_plural,
					comment: (!pending_comment.is_empty()).then(|| pending_comment.join(" ")),
					is_fuzzy,
					has_prev_msgid: prev_msgid_range.is_some(),
					flag_block: flag_block_start..msgid_start,
					msgstr_range,
				});
			}
			flag_line = None;
			prev_msgid_range = None;
			pending_comment.clear();
		}
		Self { lines, entries, obsolete }
	}

	/// Returns the index and msgid of each ordinary entry that needs a translation: one with a
	/// blank `msgstr`, or one that `msgmerge` just matched to a changed source string. See
	/// [`PoEntryLoc::has_prev_msgid`].
	pub fn needs_translation(&self) -> impl Iterator<Item = (usize, &str)> {
		self.entries
			.iter()
			.enumerate()
			.filter(|(_, e)| {
				e.msgid_plural.is_none()
					&& !e.msgid.is_empty()
					&& (e.msgstr.is_empty() || (e.is_fuzzy && e.has_prev_msgid))
			})
			.map(|(i, e)| (i, e.msgid.as_str()))
	}

	/// Returns the index, msgid, and `msgid_plural` of each plural entry that needs a translation.
	///
	/// The rule is the same as for [`Self::needs_translation`], and an entry with any blank form
	/// counts as blank, because gettext shows English for that form.
	pub fn needs_plural_translation(&self) -> impl Iterator<Item = (usize, &str, &str)> {
		self.entries.iter().enumerate().filter_map(|(i, e)| {
			let plural = e.msgid_plural.as_deref()?;
			let blank = e.msgstr_plural.is_empty() || e.msgstr_plural.iter().any(String::is_empty);
			(!e.msgid.is_empty() && (blank || (e.is_fuzzy && e.has_prev_msgid))).then_some((
				i,
				e.msgid.as_str(),
				plural,
			))
		})
	}

	/// Returns the `nplurals` value of the `Plural-Forms` header, or `None` if the header is
	/// missing or invalid. In that case, leave the file's plural entries alone rather than guess
	/// the number of forms.
	#[must_use]
	pub fn nplurals(&self) -> Option<usize> {
		let header = self.entries.iter().find(|e| e.msgid.is_empty())?;
		let start = header.msgstr.find("nplurals")?;
		let rest = header.msgstr[start..].split_once('=')?.1;
		let digits: String = rest.trim_start().chars().take_while(char::is_ascii_digit).collect();
		digits.parse().ok().filter(|n| *n > 0)
	}

	/// Writes single-string translations, each with its entry index, and flags those entries fuzzy.
	pub fn apply_all(&mut self, translations: &[(usize, String)]) {
		let owned: Vec<(usize, Translation)> =
			translations.iter().map(|(i, t)| (*i, Translation::Singular(t.clone()))).collect();
		self.apply(&owned);
	}

	/// Writes translations, each with its entry index, and flags those entries fuzzy.
	///
	/// Skips a translation whose shape doesn't match its entry, because gettext would reject the
	/// result. Pass all the translations in one call: line numbers refer to the original parse, and
	/// a first call would shift them for a second.
	pub fn apply(&mut self, translations: &[(usize, Translation)]) {
		let flagged: Vec<(usize, &Translation, bool)> = translations.iter().map(|(i, t)| (*i, t, true)).collect();
		self.apply_flagged(&flagged);
	}

	/// Writes translations like [`Self::apply`], each with whether to flag its entry fuzzy.
	///
	/// A translation that isn't flagged keeps the entry's other flags and loses any `#|` comment.
	/// Use this for a translation that a person already reviewed, such as one from an
	/// [`ObsoleteEntry`].
	pub fn apply_flagged(&mut self, translations: &[(usize, &Translation, bool)]) {
		let mut ops: Vec<(Range<usize>, Vec<String>)> = Vec::new();
		for &(idx, translated, fuzzy) in translations {
			let entry = &self.entries[idx];
			let replacement = match (translated, entry.msgid_plural.is_some()) {
				(Translation::Singular(text), false) => vec![format!("msgstr \"{}\"", pot_escape(text))],
				(Translation::Plural(forms), true) => {
					forms.iter().enumerate().map(|(n, form)| format!("msgstr[{n}] \"{}\"", pot_escape(form))).collect()
				}
				_ => continue,
			};
			let flags = if fuzzy { vec!["#, fuzzy".to_string()] } else { self.flags_without_fuzzy(entry) };
			ops.push((entry.msgstr_range.clone(), replacement));
			ops.push((entry.flag_block.clone(), flags));
		}
		ops.sort_by_key(|op| std::cmp::Reverse(op.0.start));
		for (range, replacement) in ops {
			self.lines.splice(range, replacement);
		}
	}

	fn flags_without_fuzzy(&self, entry: &PoEntryLoc) -> Vec<String> {
		let kept: Vec<&str> = self.lines[entry.flag_block.clone()]
			.iter()
			.filter_map(|line| line.trim_start().strip_prefix("#,"))
			.flat_map(|flags| flags.split(','))
			.map(str::trim)
			.filter(|flag| !flag.is_empty() && *flag != "fuzzy")
			.collect();
		if kept.is_empty() { Vec::new() } else { vec![format!("#, {}", kept.join(", "))] }
	}

	/// Returns the text of the file, with the translations that were written.
	#[must_use]
	pub fn render(&self) -> String {
		let mut out = self.lines.join("\n");
		out.push('\n');
		out
	}
}

/// Reads one obsolete entry from its `#~` lines, without the marker. The `#~|` lines start with
/// `|`, so they're passed over.
fn parse_obsolete(block: &[&str], is_fuzzy: bool) -> Option<ObsoleteEntry> {
	let mut msgid: Option<String> = None;
	let mut msgstr: Option<String> = None;
	for line in block {
		if line.starts_with("msgctxt") || line.starts_with("msgid_plural") || line.starts_with("msgstr[") {
			return None;
		}
		if let Some(rest) = line.strip_prefix("msgid ") {
			msgid = Some(po_unescape(rest));
		} else if let Some(rest) = line.strip_prefix("msgstr ") {
			msgstr = Some(po_unescape(rest));
		} else if line.starts_with('"') {
			let current = if msgstr.is_some() { msgstr.as_mut() } else { msgid.as_mut() };
			if let Some(text) = current {
				text.push_str(&po_unescape(line));
			}
		}
	}
	Some(ObsoleteEntry { msgid: msgid?, msgstr: msgstr?, is_fuzzy })
}

fn read_msgstr(lines: &[String], i: &mut usize) -> Option<(String, Range<usize>)> {
	if *i >= lines.len() || !lines[*i].trim_start().starts_with("msgstr ") {
		return None;
	}
	let start = *i;
	let rest = lines[*i].trim_start().strip_prefix("msgstr ").unwrap().to_string();
	let mut msgstr = po_unescape(&rest);
	*i += 1;
	while *i < lines.len() && lines[*i].trim_start().starts_with('"') {
		msgstr.push_str(&po_unescape(lines[*i].trim_start()));
		*i += 1;
	}
	Some((msgstr, start..*i))
}

/// Reads the run of `msgstr[N] "..."` statements after a `msgid_plural`, in file order,
/// advancing `i` past all of them.
///
/// The bracketed index is not used to place the value: gettext writes them in order from 0, and
/// trusting the order rather than the label means a file with a duplicated or missing index
/// still round-trips as the lines that are actually there, instead of silently dropping a form
/// into the wrong slot.
fn read_plural_msgstrs(lines: &[String], i: &mut usize) -> Option<(Vec<String>, Range<usize>)> {
	if *i >= lines.len() || !lines[*i].trim_start().starts_with("msgstr[") {
		return None;
	}
	let start = *i;
	let mut forms = Vec::new();
	while *i < lines.len() {
		let trimmed = lines[*i].trim_start();
		if !trimmed.starts_with("msgstr[") {
			break;
		}
		let Some((_, rest)) = trimmed.split_once(']') else {
			break;
		};
		let mut value = po_unescape(rest.trim_start());
		*i += 1;
		while *i < lines.len() && lines[*i].trim_start().starts_with('"') {
			value.push_str(&po_unescape(lines[*i].trim_start()));
			*i += 1;
		}
		forms.push(value);
	}
	Some((forms, start..*i))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn find<'a>(doc: &'a PoDocument, msgid: &str) -> &'a PoEntryLoc {
		doc.entries.iter().find(|e| e.msgid == msgid).unwrap_or_else(|| panic!("no entry for {msgid:?}"))
	}

	#[test]
	fn an_entry_carries_the_note_written_above_it() {
		let src = "#. TRANSLATORS: shown on hover
msgid \"Warning\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("shown on hover"));
	}

	/// A note can run to several lines; gettext writes one `#.` per line.
	#[test]
	fn a_note_over_several_lines_is_joined() {
		let src = "#. TRANSLATORS: first half
#. second half
msgid \"Warning\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("first half second half"));
	}

	#[test]
	fn an_entry_without_a_note_has_none() {
		let doc = PoDocument::parse(
			"msgid \"Warning\"
msgstr \"\"
",
		);
		assert_eq!(doc.entries[0].comment, None);
	}

	/// A blank line ends the entry a note belongs to, so it cannot drift onto the next one.
	#[test]
	fn a_note_does_not_reach_past_a_blank_line() {
		let src = "#. TRANSLATORS: about the first
msgid \"One\"
msgstr \"\"

msgid \"Two\"
msgstr \"\"
";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].comment.as_deref(), Some("about the first"));
		assert_eq!(doc.entries[1].comment, None);
	}

	#[test]
	fn header_entry_is_never_a_translation_candidate() {
		let doc = PoDocument::parse("msgid \"\"\nmsgstr \"\"\n\"Language: de\\n\"\n");
		assert_eq!(doc.needs_translation().count(), 0);
	}

	#[test]
	fn already_translated_entry_is_left_untouched() {
		let src = "msgid \"Cancel\"\nmsgstr \"Abbrechen\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_translation().count(), 0);
		assert_eq!(doc.render(), src);
	}

	#[test]
	fn blank_entry_with_no_flag_gets_a_fuzzy_line_inserted() {
		let src = "msgid \"Warning\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let candidates: Vec<_> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
		assert_eq!(candidates, vec![(0, "Warning".to_string())]);
		doc.apply_all(&[(0, "Warnung".to_string())]);
		assert_eq!(doc.render(), "#, fuzzy\nmsgid \"Warning\"\nmsgstr \"Warnung\"\n");
	}

	#[test]
	fn translators_comment_is_preserved_when_inserting_a_fuzzy_line() {
		let src = "#. TRANSLATORS: shown on hover\nmsgid \"Warning\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply_all(&[(0, "Warnung".to_string())]);
		assert_eq!(doc.render(), "#. TRANSLATORS: shown on hover\n#, fuzzy\nmsgid \"Warning\"\nmsgstr \"Warnung\"\n");
	}

	#[test]
	fn fuzzy_entry_with_prev_msgid_comment_is_retranslated_and_comment_dropped() {
		let src = "#, fuzzy\n#| msgid \"No pages.\"\nmsgid \"No images.\"\nmsgstr \"Sem pagina.\"\n";
		let mut doc = PoDocument::parse(src);
		let entry = find(&doc, "No images.");
		assert!(entry.is_fuzzy);
		assert!(entry.has_prev_msgid, "msgmerge-authored fuzzy match must carry a #| comment");
		assert_eq!(doc.needs_translation().count(), 1, "a genuinely stale fuzzy match is a candidate");
		let idx = doc.entries.iter().position(|e| e.msgid == "No images.").unwrap();
		doc.apply_all(&[(idx, "Sem imagens.".to_string())]);
		assert_eq!(doc.render(), "#, fuzzy\nmsgid \"No images.\"\nmsgstr \"Sem imagens.\"\n");
	}

	/// After `apply_all` translates an entry it's left `#, fuzzy` with no `#|` comment
	/// (see the test above). A later run must not treat that as a fresh candidate, that
	/// would re-translate the same unchanged text forever.
	#[test]
	fn fuzzy_entry_without_prev_msgid_comment_is_not_retranslated() {
		let src = "#, fuzzy\nmsgid \"No images.\"\nmsgstr \"Sem imagens.\"\n";
		let doc = PoDocument::parse(src);
		let entry = find(&doc, "No images.");
		assert!(entry.is_fuzzy);
		assert!(!entry.has_prev_msgid);
		assert_eq!(doc.needs_translation().count(), 0);
	}

	#[test]
	fn multiline_msgid_is_decoded_and_can_be_translated() {
		let src = "msgid \"\"\n\"Are you sure you want to remove the selected document? This will also remove \"\n\"its reading position and bookmarks.\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let entry = find(
			&doc,
			"Are you sure you want to remove the selected document? This will also remove its reading position and bookmarks.",
		);
		assert!(entry.msgstr.is_empty());
		let idx = 0;
		doc.apply_all(&[(idx, "Translated.".to_string())]);
		assert!(doc.render().contains("msgstr \"Translated.\""));
	}

	#[test]
	fn multiple_entries_patch_independently_in_one_pass() {
		let src = "msgid \"A\"\nmsgstr \"\"\n\nmsgid \"B\"\nmsgstr \"already\"\n\nmsgid \"C\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		let candidates: Vec<_> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
		assert_eq!(candidates, vec![(0, "A".to_string()), (2, "C".to_string())]);
		doc.apply_all(&[(0, "A-translated".to_string()), (2, "C-translated".to_string())]);
		let rendered = doc.render();
		assert!(rendered.contains("msgid \"A\"\nmsgstr \"A-translated\""));
		assert!(rendered.contains("msgid \"B\"\nmsgstr \"already\""));
		assert!(rendered.contains("msgid \"C\"\nmsgstr \"C-translated\""));
	}

	#[test]
	fn round_trip_escapes_quotes_and_newlines_in_translated_text() {
		let src = "msgid \"Quote\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply_all(&[(0, "She said \"hi\"\nline two".to_string())]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr \"She said \\\"hi\\\"\\nline two\""));
	}

	// A plural entry is parsed now, but stays out of the single-string API: handing one a bare
	// string would collapse msgstr[0..n] into one msgstr and break the entry.
	#[test]
	fn a_plural_entry_is_parsed_but_never_a_singular_candidate() {
		let src = "msgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries.len(), 1);
		assert_eq!(doc.entries[0].msgid_plural.as_deref(), Some("{} results"));
		assert_eq!(doc.entries[0].msgstr_plural, vec![String::new(), String::new()]);
		assert_eq!(doc.needs_translation().count(), 0, "plurals must not reach the single-string path");
		assert_eq!(doc.render(), src, "an untouched plural entry round-trips byte for byte");
	}

	#[test]
	fn a_blank_plural_entry_is_a_plural_candidate() {
		let src = "msgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let doc = PoDocument::parse(src);
		let got: Vec<_> = doc.needs_plural_translation().collect();
		assert_eq!(got, vec![(0, "{} result", "{} results")]);
	}

	// Russian has three forms. Two filled and one blank is as untranslated as none: gettext
	// falls back to the msgid for the empty slot.
	#[test]
	fn a_partly_filled_plural_entry_still_needs_translation() {
		let src =
			"msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"файл\"\nmsgstr[1] \"файла\"\nmsgstr[2] \"\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_plural_translation().count(), 1);
	}

	#[test]
	fn a_fully_translated_plural_entry_is_left_alone() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"a\"\nmsgstr[1] \"b\"\nmsgstr[2] \"c\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.needs_plural_translation().count(), 0);
	}

	#[test]
	fn applying_a_plural_writes_every_form_and_flags_it_fuzzy() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\nmsgstr[2] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(
			0,
			Translation::Plural(vec!["%d файл".to_string(), "%d файла".to_string(), "%d файлов".to_string()]),
		)]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr[0] \"%d файл\""));
		assert!(rendered.contains("msgstr[1] \"%d файла\""));
		assert!(rendered.contains("msgstr[2] \"%d файлов\""));
		assert!(rendered.contains("#, fuzzy"));
		assert!(!rendered.contains("msgstr \""), "the plural shape must survive");
	}

	// The count of forms is a property of the language, so writing three where the file had two
	// is correct, not an overflow to guard against.
	#[test]
	fn applying_more_forms_than_were_there_replaces_the_whole_run() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Plural(vec!["a".to_string(), "b".to_string(), "c".to_string()]))]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr[2] \"c\""));
		assert_eq!(rendered.matches("msgstr[").count(), 3);
	}

	// Mixing the shapes would corrupt the entry, so a mismatch is dropped rather than written.
	#[test]
	fn a_shape_mismatch_is_skipped_rather_than_written() {
		let src = "msgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Singular("nope".to_string()))]);
		assert_eq!(doc.render(), src);

		let src = "msgid \"Cancel\"\nmsgstr \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[(0, Translation::Plural(vec!["a".to_string()]))]);
		assert_eq!(doc.render(), src);
	}

	#[test]
	fn singular_and_plural_entries_patch_together_in_one_pass() {
		let src = "msgid \"Cancel\"\nmsgstr \"\"\n\nmsgid \"%d file\"\nmsgid_plural \"%d files\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n";
		let mut doc = PoDocument::parse(src);
		doc.apply(&[
			(0, Translation::Singular("Annuler".to_string())),
			(1, Translation::Plural(vec!["%d fichier".to_string(), "%d fichiers".to_string()])),
		]);
		let rendered = doc.render();
		assert!(rendered.contains("msgstr \"Annuler\""));
		assert!(rendered.contains("msgstr[0] \"%d fichier\""));
		assert!(rendered.contains("msgstr[1] \"%d fichiers\""));
	}

	#[test]
	fn nplurals_comes_from_the_header() {
		let src = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=3; plural=(n%10==1 ? 0 : 1);\\n\"\n";
		assert_eq!(PoDocument::parse(src).nplurals(), Some(3));
	}

	#[test]
	fn nplurals_is_none_without_a_usable_header() {
		assert_eq!(PoDocument::parse("msgid \"\"\nmsgstr \"\"\n").nplurals(), None);
		let broken = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=; plural=0;\\n\"\n";
		assert_eq!(PoDocument::parse(broken).nplurals(), None);
	}

	#[test]
	fn a_multiline_plural_form_is_joined() {
		let src = "msgid \"a\"\nmsgid_plural \"b\"\nmsgstr[0] \"\"\n\"joined\"\nmsgstr[1] \"x\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries[0].msgstr_plural, vec!["joined".to_string(), "x".to_string()]);
	}

	#[test]
	fn obsolete_entries_are_read_with_their_fuzzy_flag() {
		let src = "msgid \"Open\"\nmsgstr \"Öffnen\"\n\n#~ msgid \"Toggle &Bookmark\"\n#~ msgstr \"&Lesezeichen umschalten\"\n\n#, fuzzy\n#~ msgid \"\"\n#~ \"Go to \"\n#~ \"&line...\"\n#~ msgstr \"Gehe zu &Zeile...\"\n";
		let doc = PoDocument::parse(src);
		assert_eq!(doc.entries.len(), 1);
		let obsolete: Vec<_> = doc.obsolete.iter().map(|e| (e.msgid.as_str(), e.msgstr.as_str(), e.is_fuzzy)).collect();
		assert_eq!(
			obsolete,
			vec![("Toggle &Bookmark", "&Lesezeichen umschalten", false), ("Go to &line...", "Gehe zu &Zeile...", true)]
		);
	}

	#[test]
	fn an_obsolete_plural_or_context_entry_is_not_kept() {
		let src = "#~ msgid \"%d file\"\n#~ msgid_plural \"%d files\"\n#~ msgstr[0] \"%d Datei\"\n#~ msgstr[1] \"%d Dateien\"\n\n#~ msgctxt \"verb\"\n#~ msgid \"Open\"\n#~ msgstr \"Öffnen\"\n";
		assert!(PoDocument::parse(src).obsolete.is_empty());
	}

	#[test]
	fn a_previous_msgid_in_an_obsolete_entry_is_passed_over() {
		let doc = PoDocument::parse("#~| msgid \"Old\"\n#~ msgid \"New\"\n#~ msgstr \"Neu\"\n");
		assert_eq!(doc.obsolete[0].msgid, "New");
	}

	#[test]
	fn an_unflagged_result_keeps_its_other_flags_and_loses_fuzzy() {
		let src = "#, c-format\nmsgid \"%d left\"\nmsgstr \"\"\n\n#, fuzzy, c-format\n#| msgid \"%d remaining\"\nmsgid \"%d remain\"\nmsgstr \"%d übrig\"\n";
		let mut doc = PoDocument::parse(src);
		let first = Translation::Singular("noch %d".to_string());
		let second = Translation::Singular("%d bleiben".to_string());
		doc.apply_flagged(&[(0, &first, false), (1, &second, false)]);
		assert_eq!(
			doc.render(),
			"#, c-format\nmsgid \"%d left\"\nmsgstr \"noch %d\"\n\n#, c-format\nmsgid \"%d remain\"\nmsgstr \"%d bleiben\"\n"
		);
	}

	#[test]
	fn an_unflagged_result_with_no_other_flags_has_no_flag_line() {
		let mut doc = PoDocument::parse("msgid \"Open\"\nmsgstr \"\"\n");
		let open = Translation::Singular("Öffnen".to_string());
		doc.apply_flagged(&[(0, &open, false)]);
		assert_eq!(doc.render(), "msgid \"Open\"\nmsgstr \"Öffnen\"\n");
	}
}
