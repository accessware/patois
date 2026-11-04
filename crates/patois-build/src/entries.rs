use std::{collections::HashSet, fmt::Write as _};

pub struct PotEntry {
	pub(crate) msgid: String,
	pub(crate) msgid_plural: Option<String>,
	/// The `#:` line, which names the scan the string came from, such as `swift`.
	///
	/// `xgettext` runs with `--no-location`, so nothing extracted from Rust has one. That makes a
	/// reference a reliable mark of a non-Rust string, which a regeneration can carry across
	/// without also bringing back Rust strings that were renamed or deleted.
	pub(crate) reference: Option<String>,
	/// The `#.` note for translators, without the marker. Lines of a longer note are separated by
	/// newlines.
	pub(crate) comment: Option<String>,
}

/// Renders an entry with blank translations.
///
/// Keeps the `#:` line. Without it, the next regeneration discards the entry, the scan that owns it
/// adds it back, and the pot is rewritten on every other run.
pub fn pot_entry_block(entry: &PotEntry) -> String {
	// `#.` sits above `#:`, the order xgettext itself writes the two in.
	let comment = entry.comment.as_ref().map_or_else(String::new, |c| {
		c.lines().fold(String::new(), |mut out, line| {
			let _ = writeln!(out, "#. {line}");
			out
		})
	});
	let reference = entry.reference.as_ref().map_or_else(String::new, |r| format!("#: {r}\n"));
	let msgid = pot_escape(&entry.msgid);
	entry.msgid_plural.as_ref().map_or_else(
		|| format!("\n{comment}{reference}msgid \"{msgid}\"\nmsgstr \"\"\n"),
		|plural| {
			let plural = pot_escape(plural);
			format!(
				"\n{comment}{reference}msgid \"{msgid}\"\nmsgid_plural \"{plural}\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n"
			)
		},
	)
}

pub fn collect_pot_msgids(content: &str) -> HashSet<String> {
	collect_pot_entries_ordered(content).into_iter().map(|e| e.msgid).collect()
}

pub fn collect_pot_entries_ordered(content: &str) -> Vec<PotEntry> {
	let mut entries: Vec<PotEntry> = Vec::new();
	let mut current_id = String::new();
	let mut current_plural: Option<String> = None;
	let mut current_reference: Option<String> = None;
	let mut current_comment: Option<String> = None;
	// A `#:` line comes before the msgid it belongs to.
	let mut pending_reference: Option<String> = None;
	let mut pending_comment: Option<String> = None;
	let mut in_msgid = false;
	let mut in_msgid_plural = false;
	let flush = |current_id: &mut String,
	             current_plural: &mut Option<String>,
	             current_reference: &mut Option<String>,
	             current_comment: &mut Option<String>,
	             entries: &mut Vec<PotEntry>| {
		if !current_id.is_empty() || current_plural.is_some() {
			entries.push(PotEntry {
				msgid: std::mem::take(current_id),
				msgid_plural: current_plural.take(),
				reference: current_reference.take(),
				comment: current_comment.take(),
			});
		} else {
			// The header entry (empty msgid) isn't emitted, so anything picked up ahead of it
			// would otherwise leak onto the first real entry.
			*current_reference = None;
			*current_comment = None;
		}
	};
	for line in content.lines() {
		let line = line.trim();
		if let Some(rest) = line.strip_prefix("#.") {
			let text = rest.trim();
			pending_comment =
				Some(pending_comment.take().map_or_else(|| text.to_string(), |existing| format!("{existing}\n{text}")));
		} else if let Some(rest) = line.strip_prefix("#:") {
			pending_reference = Some(rest.trim().to_string());
		} else if let Some(rest) = line.strip_prefix("msgid_plural ") {
			current_plural = Some(po_unescape(rest));
			in_msgid = false;
			in_msgid_plural = true;
		} else if let Some(rest) = line.strip_prefix("msgid ") {
			flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
			current_id = po_unescape(rest);
			current_reference = pending_reference.take();
			current_comment = pending_comment.take();
			in_msgid = true;
			in_msgid_plural = false;
		} else if line.starts_with("msgstr") {
			flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
			in_msgid = false;
			in_msgid_plural = false;
		} else if line.starts_with('"') {
			if in_msgid_plural {
				if let Some(p) = current_plural.as_mut() {
					p.push_str(&po_unescape(line));
				}
			} else if in_msgid {
				current_id.push_str(&po_unescape(line));
			}
		}
	}
	flush(&mut current_id, &mut current_plural, &mut current_reference, &mut current_comment, &mut entries);
	entries
}

pub fn pot_escape(s: &str) -> String {
	let mut out = String::with_capacity(s.len());
	for c in s.chars() {
		match c {
			'"' => out.push_str("\\\""),
			'\\' => out.push_str("\\\\"),
			'\n' => out.push_str("\\n"),
			'\t' => out.push_str("\\t"),
			c => out.push(c),
		}
	}
	out
}

pub fn po_unescape(s: &str) -> String {
	let s = s.trim();
	if s.len() < 2 || !s.starts_with('"') || !s.ends_with('"') {
		return String::new();
	}
	let inner = &s[1..s.len() - 1];
	let mut out = String::with_capacity(inner.len());
	let mut chars = inner.chars();
	while let Some(c) = chars.next() {
		if c == '\\' {
			match chars.next() {
				Some('n') => out.push('\n'),
				Some('t') => out.push('\t'),
				Some('"') => out.push('"'),
				Some(c) => {
					out.push('\\');
					if c != '\\' {
						out.push(c);
					}
				}
				None => out.push('\\'),
			}
		} else {
			out.push(c);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn pot_escape_roundtrip() {
		let s = "say \"hi\" and \\bye\nnewline";
		let escaped = pot_escape(s);
		assert_eq!(escaped, r#"say \"hi\" and \\bye\nnewline"#);
		assert_eq!(po_unescape(&format!("\"{escaped}\"")), s);
	}

	#[test]
	fn collect_msgids_finds_existing() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n\nmsgid \"OK\"\nmsgstr \"OK\"\n";
		let ids = collect_pot_msgids(pot);
		assert!(ids.contains("Cancel"));
		assert!(ids.contains("OK"));
	}

	#[test]
	fn collect_pot_entries_captures_plural_shape() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"{} result\"\nmsgid_plural \"{} results\"\nmsgstr[0] \"\"\nmsgstr[1] \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let plural_entry = entries.iter().find(|e| e.msgid == "{} result").unwrap();
		assert_eq!(plural_entry.msgid_plural.as_deref(), Some("{} results"));
		let singular_entry = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(singular_entry.msgid_plural, None);
	}

	// The `#:` line sits above its msgid, so it has to be held and attached to the entry that
	// follows rather than the one just finished.
	#[test]
	fn collect_pot_entries_attaches_a_reference_to_the_entry_below_it() {
		let pot = "msgid \"\"\nmsgstr \"\"\n\nmsgid \"From Rust\"\nmsgstr \"\"\n\n#: swift\nmsgid \"From Swift\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let rust = entries.iter().find(|e| e.msgid == "From Rust").unwrap();
		assert_eq!(rust.reference, None, "the reference belongs to the entry after it, not before");
		let swift = entries.iter().find(|e| e.msgid == "From Swift").unwrap();
		assert_eq!(swift.reference.as_deref(), Some("swift"));
	}

	// The header's empty msgid is parsed but never emitted as an entry, so a reference read
	// before it has nowhere to go and must not drift onto the first real string.
	#[test]
	fn a_reference_above_the_header_does_not_leak_onto_the_first_entry() {
		let pot = "#: swift\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let cancel = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(cancel.reference, None);
	}

	#[test]
	fn pot_entry_block_round_trips_a_reference() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: Some("kt".to_string()),
			comment: None,
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#: kt\nmsgid \"Continue\""));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].reference.as_deref(), Some("kt"));
	}
	// A note has to survive a preserve-and-rewrite cycle: `preserve_foreign_entries` parses the
	// old pot and re-renders what it finds, so anything the parser drops is gone for good.
	#[test]
	fn pot_entry_block_round_trips_a_comment() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: Some("kt".to_string()),
			comment: Some("TRANSLATORS: on the last onboarding page".to_string()),
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#. TRANSLATORS: on the last onboarding page\n#: kt\nmsgid"));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].comment.as_deref(), Some("TRANSLATORS: on the last onboarding page"));
	}

	#[test]
	fn a_comment_spanning_several_lines_round_trips() {
		let entry = PotEntry {
			msgid: "Continue".to_string(),
			msgid_plural: None,
			reference: None,
			comment: Some("TRANSLATORS: first half\nsecond half".to_string()),
		};
		let rendered = pot_entry_block(&entry);
		assert!(rendered.contains("#. TRANSLATORS: first half\n#. second half\n"));
		let parsed = collect_pot_entries_ordered(&rendered);
		assert_eq!(parsed[0].comment.as_deref(), Some("TRANSLATORS: first half\nsecond half"));
	}

	// Same trap as the reference: a `#.` read before the header entry has nowhere to go.
	#[test]
	fn a_comment_above_the_header_does_not_leak_onto_the_first_entry() {
		let pot = "#. TRANSLATORS: stray\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let entries = collect_pot_entries_ordered(pot);
		let cancel = entries.iter().find(|e| e.msgid == "Cancel").unwrap();
		assert_eq!(cancel.comment, None);
	}
}
