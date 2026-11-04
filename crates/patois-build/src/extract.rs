//! Extracts strings from Swift and Kotlin source. This is a small scanner rather than another
//! `xgettext` run because those languages need no sanitizing and their call shapes are simple.

use std::{
	collections::{HashMap, HashSet},
	error, fs,
	path::{Path, PathBuf},
};

use crate::entries::{PotEntry, collect_pot_msgids, po_unescape, pot_entry_block};

fn collect_source_files(dir: &Path, extension: &str, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn error::Error>> {
	if !dir.is_dir() {
		return Ok(());
	}
	let mut paths = fs::read_dir(dir)?.map(|entry| entry.map(|e| e.path())).collect::<Result<Vec<_>, _>>()?;
	// `fs::read_dir` guarantees no order, and the order decides the pot's entries and which
	// occurrence of a string gives its translator note.
	paths.sort();
	for path in paths {
		if path.is_dir() {
			collect_source_files(&path, extension, files)?;
		} else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
			files.push(path);
		}
	}
	Ok(())
}

/// Adds the strings of `t("...")` and `nt("...", "...", n)` calls in other languages to an existing
/// `.pot` file.
///
/// Scans the files with the given `extension`, such as `swift` or `kt`, and doesn't need
/// `xgettext`. Each added entry is marked with `extension`, so [`gen_pot`](crate::gen_pot) keeps it
/// and [`prune_pot_from_source_dirs`] can remove it. If a string appears more than once, its first
/// occurrence gives its translator note. Does nothing if `pot_file` doesn't exist.
///
/// # Errors
///
/// Returns an error if a file can't be read or the pot can't be written.
pub fn extend_pot_from_source_dirs(
	dirs: &[impl AsRef<Path>],
	extension: &str,
	pot_file: impl AsRef<Path>,
) -> Result<(), Box<dyn error::Error>> {
	let pot_file = pot_file.as_ref();
	if !pot_file.exists() {
		return Ok(());
	}
	let mut files: Vec<PathBuf> = Vec::new();
	for dir in dirs {
		collect_source_files(dir.as_ref(), extension, &mut files)?;
	}
	if files.is_empty() {
		return Ok(());
	}

	// `t()` and `nt()` share one set because gettext keys a catalog on the singular text.
	let mut new_entries: Vec<PotEntry> = Vec::new();
	let mut seen_in_scan: HashSet<String> = HashSet::new();
	for file in &files {
		let content = fs::read_to_string(file)?;
		for (s, comment) in extract_t_strings(&content) {
			if seen_in_scan.insert(s.clone()) {
				new_entries.push(PotEntry {
					msgid: s,
					msgid_plural: None,
					reference: Some(extension.to_string()),
					comment,
				});
			}
		}
		for (singular, plural, comment) in extract_nt_strings(&content) {
			if seen_in_scan.insert(singular.clone()) {
				new_entries.push(PotEntry {
					msgid: singular,
					msgid_plural: Some(plural),
					reference: Some(extension.to_string()),
					comment,
				});
			}
		}
	}
	if new_entries.is_empty() {
		return Ok(());
	}

	let existing = fs::read_to_string(pot_file)?;
	let existing_ids = collect_pot_msgids(&existing);

	let notes: HashMap<&str, Option<&str>> =
		new_entries.iter().map(|e| (e.msgid.as_str(), e.comment.as_deref())).collect();
	let refreshed = refresh_foreign_comments(&existing, &notes, extension);
	// Pruning works per reference, so a string that two front ends use needs a reference from each,
	// or dropping it from one front end takes it from the other too.
	let scanned: HashSet<&str> = new_entries.iter().map(|e| e.msgid.as_str()).collect();
	let refreshed = add_reference_to_shared_entries(&refreshed, &scanned, extension);

	let mut additions = String::new();
	for entry in &new_entries {
		if !existing_ids.contains(&entry.msgid) {
			additions.push_str(&pot_entry_block(entry));
		}
	}
	if !additions.is_empty() || refreshed != existing {
		fs::write(pot_file, format!("{refreshed}{additions}"))?;
	}
	Ok(())
}

/// Removes the entries that [`extend_pot_from_source_dirs`] added whose strings are no longer in
/// the sources, and returns those strings.
///
/// A regeneration copies these entries across rather than rebuilding them from source, so without
/// this function a renamed or deleted string stays in the pot, and translators keep translating
/// it. Entries from other scans, and entries from the Rust scan, stay.
///
/// # Errors
///
/// Returns an error if a source directory or the pot cannot be read, or the pot cannot be
/// written.
pub fn prune_pot_from_source_dirs(
	dirs: &[impl AsRef<Path>],
	extension: &str,
	pot_file: impl AsRef<Path>,
) -> Result<Vec<String>, Box<dyn error::Error>> {
	let pot_file = pot_file.as_ref();
	if !pot_file.exists() {
		return Ok(Vec::new());
	}
	let mut files: Vec<PathBuf> = Vec::new();
	for dir in dirs {
		collect_source_files(dir.as_ref(), extension, &mut files)?;
	}
	// A directory that moved would otherwise read as every string having been deleted.
	if files.is_empty() {
		return Ok(Vec::new());
	}
	// Searching the sources for the text would also keep a string whose words appear in a comment.
	let mut alive: HashSet<String> = HashSet::new();
	for file in &files {
		let content = fs::read_to_string(file)?;
		for (s, _) in extract_t_strings(&content) {
			alive.insert(s);
		}
		for (singular, _, _) in extract_nt_strings(&content) {
			alive.insert(singular);
		}
	}
	let existing = fs::read_to_string(pot_file)?;
	let reference = format!("#: {extension}");
	let mut removed: Vec<String> = Vec::new();
	let kept: Vec<String> = existing
		.split("\n\n")
		.filter_map(|block| {
			let lines: Vec<&str> = block.lines().collect();
			if !lines.iter().any(|l| l.trim() == reference) {
				return Some(block.to_string());
			}
			let Some(msgid) = block_msgid(&lines) else {
				return Some(block.to_string());
			};
			if alive.contains(&msgid) {
				return Some(block.to_string());
			}
			// Another front end may still use the string, so only this scan's reference goes.
			if !lines.iter().any(|l| l.trim().starts_with("#: ") && l.trim() != reference) {
				removed.push(msgid);
				return None;
			}
			let trailing = if block.ends_with('\n') { "\n" } else { "" };
			let rebuilt: Vec<&str> = lines.into_iter().filter(|l| l.trim() != reference).collect();
			Some(format!("{}{trailing}", rebuilt.join("\n")))
		})
		.collect();
	let rebuilt = kept.join("\n\n");
	if rebuilt != existing {
		fs::write(pot_file, rebuilt)?;
	}
	Ok(removed)
}

/// Adds this scan's `#:` reference to entries that another scan wrote, so that
/// [`prune_pot_from_source_dirs`] can tell a string nobody uses from one that a single front end
/// stopped using.
fn add_reference_to_shared_entries(content: &str, scanned: &HashSet<&str>, extension: &str) -> String {
	let reference = format!("#: {extension}");
	content
		.split("\n\n")
		.map(|block| {
			let trailing = if block.ends_with('\n') { "\n" } else { "" };
			let lines: Vec<&str> = block.lines().collect();
			// The Rust scan rebuilds entries with no reference, so an added one wouldn't survive.
			if lines.iter().any(|l| l.trim() == reference) || !lines.iter().any(|l| l.trim().starts_with("#: ")) {
				return block.to_string();
			}
			let Some(msgid) = block_msgid(&lines) else {
				return block.to_string();
			};
			if !scanned.contains(msgid.as_str()) {
				return block.to_string();
			}
			let Some(at) = lines.iter().position(|l| l.trim().starts_with("#: ")) else {
				return block.to_string();
			};
			let mut rebuilt: Vec<&str> = lines;
			rebuilt.insert(at, reference.as_str());
			format!("{}{trailing}", rebuilt.join("\n"))
		})
		.collect::<Vec<String>>()
		.join("\n\n")
}

/// Updates the `#.` notes on this scan's entries.
///
/// New entries are the only ones written from source, so without this function a note added or
/// changed after its string first reached the pot would never reach the catalog.
fn refresh_foreign_comments(content: &str, notes: &HashMap<&str, Option<&str>>, extension: &str) -> String {
	let reference = format!("#: {extension}");
	// A newline inside a string is escaped, so an empty line is always an entry boundary.
	content
		.split("\n\n")
		.map(|block| {
			// `lines()` drops a trailing newline, which on the final block is the file's own.
			let trailing = if block.ends_with('\n') { "\n" } else { "" };
			let lines: Vec<&str> = block.lines().collect();
			if !lines.iter().any(|l| l.trim() == reference) {
				return block.to_string();
			}
			let Some(msgid) = block_msgid(&lines) else {
				return block.to_string();
			};
			let Some(note) = notes.get(msgid.as_str()) else {
				return block.to_string();
			};
			let mut rebuilt: Vec<String> = Vec::with_capacity(lines.len() + 1);
			let mut rest = lines.as_slice();
			while let Some((first, tail)) = rest.split_first() {
				if first.trim().is_empty() {
					rebuilt.push((*first).to_string());
					rest = tail;
				} else {
					break;
				}
			}
			if let Some(text) = note {
				rebuilt.extend(text.lines().map(|line| format!("#. {line}")));
			}
			rebuilt.extend(rest.iter().filter(|l| !l.starts_with("#.")).map(|l| (*l).to_string()));
			format!("{}{trailing}", rebuilt.join("\n"))
		})
		.collect::<Vec<_>>()
		.join("\n\n")
}

fn block_msgid(lines: &[&str]) -> Option<String> {
	let start = lines.iter().position(|l| l.starts_with("msgid "))?;
	let mut msgid = po_unescape(lines[start].trim_start_matches("msgid "));
	for line in &lines[start + 1..] {
		if line.starts_with('"') {
			msgid.push_str(&po_unescape(line));
		} else {
			break;
		}
	}
	Some(msgid)
}

/// Reads the string literal at `pos`, after any whitespace, and returns it decoded with the index
/// past its closing quote. Returns `None` if there's no literal or it isn't terminated.
fn read_string_literal(chars: &[char], mut pos: usize) -> Option<(String, usize)> {
	let n = chars.len();
	while pos < n && chars[pos].is_ascii_whitespace() {
		pos += 1;
	}
	if pos >= n || chars[pos] != '"' {
		return None;
	}
	pos += 1;
	let mut s = String::new();
	loop {
		if pos >= n {
			return None;
		}
		match chars[pos] {
			'"' => return Some((s, pos + 1)),
			'\\' if pos + 1 < n => {
				pos += 1;
				match chars[pos] {
					'n' => s.push('\n'),
					't' => s.push('\t'),
					'"' => s.push('"'),
					'\\' => s.push('\\'),
					'u' if let Some((c, next)) = read_unicode_escape(chars, pos) => {
						s.push(c);
						pos = next - 1;
					}
					c => {
						s.push('\\');
						s.push(c);
					}
				}
				pos += 1;
			}
			'\n' | '\r' => return None,
			c => {
				s.push(c);
				pos += 1;
			}
		}
	}
}

/// Decodes the `\u{…}` (Rust, Swift) or `\uXXXX` (Kotlin) escape whose `u` is at `pos`, and returns
/// the character with the index past the escape.
pub fn read_unicode_escape(chars: &[char], pos: usize) -> Option<(char, usize)> {
	let (start, end, next) = if chars.get(pos + 1) == Some(&'{') {
		let close = pos + 2 + chars.get(pos + 2..)?.iter().position(|&c| c == '}')?;
		(pos + 2, close, close + 1)
	} else {
		(pos + 1, pos + 5, pos + 5)
	};
	let hex: String = chars.get(start..end)?.iter().collect();
	if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
		return None;
	}
	Some((char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?, next))
}

/// Matches the `--add-comments=TRANSLATORS` that the Rust scan passes to `xgettext`.
const COMMENT_TAG: &str = "TRANSLATORS";

/// How many lines may sit between a note and its call, as in:
///
/// ```text
/// // TRANSLATORS: Section heading
/// Text(
///     t("General"),
/// ```
///
/// Only lines with no string literal count. Such a line can't hold another message, so the note
/// can't belong to anything else.
const MAX_SKIPPED_WRAPPER_LINES: usize = 3;

fn line_of_each_char(chars: &[char]) -> Vec<usize> {
	let mut out = Vec::with_capacity(chars.len() + 1);
	let mut line = 0;
	for &c in chars {
		out.push(line);
		if c == '\n' {
			line += 1;
		}
	}
	out.push(line);
	out
}

/// Returns the `TRANSLATORS:` note for the call on `line_index`, if it has one.
///
/// A blank line ends the search, so that an unrelated note further up can't attach to this call.
fn translators_comment_before(lines: &[&str], line_index: usize) -> Option<String> {
	let mut cursor = line_index;
	let mut skipped = 0;
	while cursor > 0 {
		let above = lines[cursor - 1].trim();
		if above.starts_with("//") {
			break;
		}
		if above.is_empty() || above.contains('"') || skipped == MAX_SKIPPED_WRAPPER_LINES {
			return None;
		}
		skipped += 1;
		cursor -= 1;
	}
	let mut block: Vec<&str> = Vec::new();
	while cursor > 0 {
		let Some(text) = lines[cursor - 1].trim().strip_prefix("//") else {
			break;
		};
		block.push(text.trim());
		cursor -= 1;
	}
	block.reverse();
	if block.first()?.starts_with(COMMENT_TAG) { Some(block.join("\n")) } else { None }
}

// `preceded_by_ident` keeps a name that ends in `t`, such as `stateDescription(`, from matching.
fn extract_t_strings(content: &str) -> Vec<(String, Option<String>)> {
	let chars: Vec<char> = content.chars().collect();
	let lines: Vec<&str> = content.lines().collect();
	let line_of = line_of_each_char(&chars);
	let n = chars.len();
	let mut out: Vec<(String, Option<String>)> = Vec::new();
	let mut i = 0;
	while i < n {
		if chars[i] == 't' && i + 1 < n && chars[i + 1] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident && let Some((s, next)) = read_string_literal(&chars, i + 2) {
				if !s.is_empty() {
					out.push((s, translators_comment_before(&lines, line_of[i])));
				}
				i = next;
				continue;
			}
		}
		i += 1;
	}
	out
}

fn extract_nt_strings(content: &str) -> Vec<(String, String, Option<String>)> {
	let chars: Vec<char> = content.chars().collect();
	let lines: Vec<&str> = content.lines().collect();
	let line_of = line_of_each_char(&chars);
	let n = chars.len();
	let mut out: Vec<(String, String, Option<String>)> = Vec::new();
	let mut i = 0;
	while i < n {
		if chars[i] == 'n' && i + 1 < n && chars[i + 1] == 't' && i + 2 < n && chars[i + 2] == '(' {
			let preceded_by_ident = i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_');
			if !preceded_by_ident && let Some((singular, after_singular)) = read_string_literal(&chars, i + 3) {
				let mut j = after_singular;
				while j < n && (chars[j].is_ascii_whitespace() || chars[j] == ',') {
					j += 1;
				}
				if let Some((plural, after_plural)) = read_string_literal(&chars, j) {
					if !singular.is_empty() && !plural.is_empty() {
						out.push((singular, plural, translators_comment_before(&lines, line_of[i])));
					}
					i = after_plural;
					continue;
				}
			}
		}
		i += 1;
	}
	out
}
#[cfg(test)]
mod tests {
	use super::*;

	fn t_texts(src: &str) -> Vec<String> {
		extract_t_strings(src).into_iter().map(|(s, _)| s).collect()
	}

	fn nt_pairs(src: &str) -> Vec<(String, String)> {
		extract_nt_strings(src).into_iter().map(|(a, b, _)| (a, b)).collect()
	}

	#[test]
	fn extract_simple() {
		assert_eq!(t_texts(r#"Button(t("Cancel")) { dismiss() }"#), vec!["Cancel"]);
	}

	#[test]
	fn extract_multiple() {
		assert_eq!(t_texts(r#"Text(t("Find")) Text(t("Cancel"))"#), vec!["Find", "Cancel"]);
	}

	#[test]
	fn extract_escaped_quote() {
		assert_eq!(t_texts(r#"t("say \"hi\"")"#), vec!["say \"hi\""]);
	}

	#[test]
	fn extract_backslash_escape() {
		// Swift source on disk: t("Regular expression (\\1 = first capture group)")
		// Two actual backslash chars in the file → decoded to one backslash in the msgid.
		let src = "t(\"Regular expression (\\\\1 = first capture group)\")";
		assert_eq!(t_texts(src), vec!["Regular expression (\\1 = first capture group)"]);
	}

	#[test]
	fn skip_ident_suffix_t() {
		let src = r#"putText("bad") t("good")"#;
		let got = t_texts(src);
		assert!(got.contains(&"good".to_string()));
		assert!(!got.contains(&"bad".to_string()));
	}

	#[test]
	fn unicode_passthrough() {
		let src = "t(\"Search\u{2026}\")";
		assert_eq!(t_texts(src), vec!["Search\u{2026}"]);
	}

	#[test]
	fn unicode_escapes_are_decoded() {
		assert_eq!(t_texts(r#"t("1.5\u{00d7}") t("“quoted”")"#), vec!["1.5\u{d7}", "\u{201c}quoted\u{201d}"]);
	}

	#[test]
	fn extract_nt_finds_singular_and_plural() {
		let src = r#"Text(nt("{} result", "{} results", count))"#;
		assert_eq!(nt_pairs(src), vec![("{} result".to_string(), "{} results".to_string())]);
	}

	#[test]
	fn extract_nt_ignores_nt_suffix() {
		let src = r#"xnt("bad", "worse", n) nt("good", "goods", n)"#;
		assert_eq!(nt_pairs(src), vec![("good".to_string(), "goods".to_string())]);
	}
	#[test]
	fn note_directly_above_a_call_is_attached() {
		let src = "// TRANSLATORS: Button that closes the sheet\nText(t(\"Done\"))";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: Button that closes the sheet"));
	}

	// The note is written above the whole widget, so the line opening it sits in between.
	#[test]
	fn note_carries_over_a_wrapper_line() {
		let src = "// TRANSLATORS: Section heading\nText(\n\tt(\"General\"),\n)";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: Section heading"));
	}

	#[test]
	fn an_ordinary_comment_is_not_a_note() {
		let src = "// this explains the code, not the string\nt(\"Done\")";
		assert_eq!(extract_t_strings(src)[0].1, None);
	}

	// A blank line is what separates a note from code it was not written for.
	#[test]
	fn a_blank_line_detaches_the_note() {
		let src = "// TRANSLATORS: for something else\n\nt(\"Done\")";
		assert_eq!(extract_t_strings(src)[0].1, None);
	}

	// The note above the first call must not also land on the second.
	#[test]
	fn a_note_does_not_leak_onto_the_next_call() {
		let src = "// TRANSLATORS: about A\nt(\"A\")\nt(\"B\")";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: about A"));
		assert_eq!(got[1].1, None);
	}

	#[test]
	fn a_note_spanning_several_lines_is_joined() {
		let src = "// TRANSLATORS: first half\n// second half\nt(\"Done\")";
		let got = extract_t_strings(src);
		assert_eq!(got[0].1.as_deref(), Some("TRANSLATORS: first half\nsecond half"));
	}

	#[test]
	fn nt_calls_pick_up_their_note_too() {
		let src = "// TRANSLATORS: {} is the match count\nnt(\"{} result\", \"{} results\", n)";
		let got = extract_nt_strings(src);
		assert_eq!(got[0].2.as_deref(), Some("TRANSLATORS: {} is the match count"));
	}
	struct SourceDir {
		dir: std::path::PathBuf,
	}

	impl SourceDir {
		fn with(name: &str, contents: &str) -> Self {
			let dir = std::env::temp_dir().join(format!("patois-prune-{}-{}", std::process::id(), name));
			fs::create_dir_all(&dir).unwrap();
			fs::write(dir.join("Screen.kt"), contents).unwrap();
			Self { dir }
		}

		fn pot(&self, contents: &str) -> std::path::PathBuf {
			let path = self.dir.join("messages.pot");
			fs::write(&path, contents).unwrap();
			path
		}
	}

	impl Drop for SourceDir {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.dir);
		}
	}

	#[test]
	fn a_string_another_scan_already_wrote_gains_this_scan_s_reference() {
		let src = SourceDir::with(
			"shared-extend",
			"Text(t(\"Settings exported\"))
",
		);
		let pot = src.pot(
			"msgid \"\"
msgstr \"\"

#: swift
msgid \"Settings exported\"
msgstr \"\"
",
		);
		extend_pot_from_source_dirs(&[&src.dir], "kt", &pot).unwrap();
		let after = fs::read_to_string(&pot).unwrap();
		assert!(
			after.contains(
				"#: kt
#: swift
msgid \"Settings exported\""
			),
			"got:
{after}"
		);
		assert_eq!(after.matches("msgid \"Settings exported\"").count(), 1, "and not a second copy of it");
	}

	#[test]
	fn a_shared_entry_keeps_the_other_scan_s_reference() {
		let src = SourceDir::with(
			"shared-prune",
			"Text(t(\"Still called\"))
",
		);
		let pot = src.pot(
			"msgid \"\"
msgstr \"\"

#: kt
msgid \"Still called\"
msgstr \"\"

#: kt
#: swift
msgid \"Shared\"
msgstr \"\"
",
		);
		let removed = prune_pot_from_source_dirs(&[&src.dir], "kt", &pot).unwrap();
		assert!(removed.is_empty(), "the entry is still used elsewhere, so nothing was removed");
		let after = fs::read_to_string(&pot).unwrap();
		assert!(after.contains("msgid \"Shared\""), "the string itself stays");
		assert!(
			after.contains(
				"#: swift
msgid \"Shared\""
			),
			"under the other scan's reference: {after}"
		);
		assert!(
			!after.contains(
				"#: kt
#: swift"
			),
			"and no longer under this one: {after}"
		);
	}

	#[test]
	fn an_entry_whose_call_is_gone_is_removed() {
		let src = SourceDir::with("gone", "Text(t(\"Still called\"))\n// Gone now: mentioned only in a comment\n");
		let pot = src.pot("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust owned\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Still called\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Gone now\"\nmsgstr \"\"\n\n#: swift\nmsgid \"Another platform\"\nmsgstr \"\"\n");
		let removed = prune_pot_from_source_dirs(&[&src.dir], "kt", &pot).unwrap();
		assert_eq!(removed, vec!["Gone now".to_string()]);
		let after = fs::read_to_string(&pot).unwrap();
		assert!(after.contains("Still called"), "the call that is still there keeps its entry");
		assert!(!after.contains("Gone now"), "the one that is not should have gone");
	}

	/// Entries belonging to another scan, and the xgettext-owned ones with no reference at
	/// all, are none of this function's business.
	#[test]
	fn entries_owned_by_others_are_left_alone() {
		let src = SourceDir::with("others", "Text(t(\"Still called\"))\n// Gone now: mentioned only in a comment\n");
		let pot = src.pot("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust owned\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Still called\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Gone now\"\nmsgstr \"\"\n\n#: swift\nmsgid \"Another platform\"\nmsgstr \"\"\n");
		prune_pot_from_source_dirs(&[&src.dir], "kt", &pot).unwrap();
		let after = fs::read_to_string(&pot).unwrap();
		assert!(after.contains("Rust owned"), "no reference means xgettext owns it");
		assert!(after.contains("Another platform"), "a different reference means a different scan owns it");
	}

	/// The text appears in the file, but only in a comment. Searching the source for it
	/// would keep the entry; asking the extractor what the file actually declares does not.
	#[test]
	fn a_string_that_only_appears_in_a_comment_does_not_keep_an_entry() {
		let src = SourceDir::with("comment", "Text(t(\"Still called\"))\n// Gone now: mentioned only in a comment\n");
		let pot = src.pot("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust owned\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Still called\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Gone now\"\nmsgstr \"\"\n\n#: swift\nmsgid \"Another platform\"\nmsgstr \"\"\n");
		let removed = prune_pot_from_source_dirs(&[&src.dir], "kt", &pot).unwrap();
		assert!(removed.contains(&"Gone now".to_string()));
	}

	/// A directory that moved reads as an empty scan, which must not be taken as every
	/// string having been deleted at once.
	#[test]
	fn no_sources_at_all_removes_nothing() {
		let src = SourceDir::with("empty", "");
		let pot = src.pot("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust owned\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Still called\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Gone now\"\nmsgstr \"\"\n\n#: swift\nmsgid \"Another platform\"\nmsgstr \"\"\n");
		let missing = src.dir.join("not-here");
		let removed = prune_pot_from_source_dirs(&[&missing], "kt", &pot).unwrap();
		assert!(removed.is_empty());
		assert_eq!(
			fs::read_to_string(&pot).unwrap(),
			"msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust owned\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Still called\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Gone now\"\nmsgstr \"\"\n\n#: swift\nmsgid \"Another platform\"\nmsgstr \"\"\n"
		);
	}

	fn pot_with_a_kt_entry() -> String {
		String::from("msgid \"\"\nmsgstr \"\"\n\nmsgid \"Rust\"\nmsgstr \"\"\n\n#: kt\nmsgid \"Cancel\"\nmsgstr \"\"\n")
	}

	// The msgid landed in the pot before anyone wrote a note for it, and only this refresh can
	// carry the note across afterwards: the appending path skips msgids the pot already has.
	#[test]
	fn a_note_is_backfilled_onto_an_entry_already_in_the_pot() {
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: dismisses the sheet"))]);
		let got = refresh_foreign_comments(&pot_with_a_kt_entry(), &notes, "kt");
		assert!(got.contains("#. TRANSLATORS: dismisses the sheet\n#: kt\nmsgid \"Cancel\""));
	}

	#[test]
	fn a_reworded_note_replaces_the_old_one() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: old wording\n#: kt");
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: new wording"))]);
		let got = refresh_foreign_comments(&before, &notes, "kt");
		assert!(got.contains("TRANSLATORS: new wording"));
		assert!(!got.contains("TRANSLATORS: old wording"));
	}

	#[test]
	fn a_deleted_note_is_removed_from_the_entry() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: gone now\n#: kt");
		let notes = HashMap::from([("Cancel", None)]);
		let got = refresh_foreign_comments(&before, &notes, "kt");
		assert!(!got.contains("#."));
		assert!(got.contains("#: kt\nmsgid \"Cancel\""));
	}

	// Entries xgettext owns carry no `#:` at all, and another scan's carry a different one.
	#[test]
	fn entries_this_scan_does_not_own_are_left_alone() {
		let before = pot_with_a_kt_entry();
		let notes = HashMap::from([("Rust", Some("TRANSLATORS: not yours"))]);
		assert_eq!(refresh_foreign_comments(&before, &notes, "kt"), before);
		let swift_notes = HashMap::from([("Cancel", Some("TRANSLATORS: not yours"))]);
		assert_eq!(refresh_foreign_comments(&before, &swift_notes, "swift"), before);
	}

	// `lines()` drops it, and on the last block that is the file's own trailing newline.
	#[test]
	fn the_files_trailing_newline_survives_a_refresh() {
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: dismisses the sheet"))]);
		let got = refresh_foreign_comments(&pot_with_a_kt_entry(), &notes, "kt");
		assert!(got.ends_with('\n'));
	}

	// Rewriting a block that needs no change would churn the pot and bump POT-Creation-Date.
	#[test]
	fn a_pot_that_is_already_current_is_returned_untouched() {
		let before = pot_with_a_kt_entry().replace("#: kt", "#. TRANSLATORS: same as ever\n#: kt");
		let notes = HashMap::from([("Cancel", Some("TRANSLATORS: same as ever"))]);
		assert_eq!(refresh_foreign_comments(&before, &notes, "kt"), before);
	}
}
