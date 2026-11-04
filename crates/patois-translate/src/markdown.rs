/// Puts the inline code spans of the source back.
///
/// The prompt says that code spans are verbatim, and the model mostly obeys, but `Alt+Left` came
/// back as `Alt+Gauche` in French, which isn't a key anyone can press.
///
/// If the span counts differ, the translation stays as it came back, because the nth span in it
/// may not be the nth in the source.
pub fn restore_code_spans(source: &str, translated: &str) -> String {
	let source_spans = code_spans(source);
	let translated_spans = code_spans(translated);
	if source_spans.len() != translated_spans.len() {
		return translated.to_string();
	}
	let mut out = String::with_capacity(translated.len());
	let mut last = 0;
	for ((range, _), (_, original)) in translated_spans.into_iter().zip(source_spans) {
		out.push_str(&translated[last..range.start]);
		out.push_str(original);
		last = range.end;
	}
	out.push_str(&translated[last..]);
	out
}

// Stops at a newline, so that a stray backtick can't swallow the rest of the document.
fn code_spans(text: &str) -> Vec<(std::ops::Range<usize>, &str)> {
	let bytes = text.as_bytes();
	let mut out = Vec::new();
	let mut i = 0;
	while i < bytes.len() {
		if bytes[i] != b'`' {
			i += 1;
			continue;
		}
		let start = i + 1;
		let mut j = start;
		while j < bytes.len() && bytes[j] != b'`' && bytes[j] != b'\n' {
			j += 1;
		}
		if j < bytes.len() && bytes[j] == b'`' && j > start {
			out.push((start..j, &text[start..j]));
			i = j + 1;
		} else {
			i += 1;
		}
	}
	out
}

/// Splits Markdown at `##` headings. The readme sync hashes these sections, so an unchanged section
/// must be cut the same way each time.
pub fn split_sections(markdown: &str) -> Vec<String> {
	split_at_headings(markdown, 2)
}

fn split_at_headings(markdown: &str, level: usize) -> Vec<String> {
	let marker = format!("{} ", "#".repeat(level));
	let mut pieces: Vec<String> = Vec::new();
	let mut current = String::new();
	for line in markdown.lines() {
		if line.starts_with(&marker) && !current.trim().is_empty() {
			pieces.push(current.trim_end().to_string());
			current = String::new();
		}
		current.push_str(line);
		current.push('\n');
	}
	if !current.trim().is_empty() {
		pieces.push(current.trim_end().to_string());
	}
	pieces
}

const MAX_HEADING_LEVEL: usize = 6;

/// What a translated section must keep from its source: the same headings at the same levels, and
/// the same number of list items.
///
/// Models do leave things out, and nothing can tell prose worded differently from prose left out.
/// Structure is the part of a translation that must not change, so it's the part worth checking.
#[derive(Debug, PartialEq, Eq)]
pub struct Outline {
	headings: Vec<usize>,
	list_items: usize,
	fences: usize,
}

pub fn structure_mismatch(source: &str, translated: &str) -> Option<String> {
	if translated.trim().is_empty() {
		return Some("the translation is empty".to_string());
	}
	let want = outline(source);
	let got = outline(translated);
	if want == got {
		return None;
	}
	if want.headings != got.headings {
		return Some(format!("headings are {:?} in English and {:?} in the translation", want.headings, got.headings));
	}
	if want.list_items != got.list_items {
		return Some(format!("{} list items in English and {} in the translation", want.list_items, got.list_items));
	}
	Some(format!("{} code fences in English and {} in the translation", want.fences, got.fences))
}

fn outline(markdown: &str) -> Outline {
	let mut headings = Vec::new();
	let mut list_items = 0;
	let mut fences = 0;
	let mut in_fence = false;
	for line in markdown.lines() {
		let trimmed = line.trim_start();
		if trimmed.starts_with("```") {
			fences += 1;
			in_fence = !in_fence;
			continue;
		}
		if in_fence {
			continue;
		}
		if let Some(level) = heading_level(trimmed) {
			headings.push(level);
		} else if trimmed.starts_with("* ") || trimmed.starts_with("- ") {
			list_items += 1;
		}
	}
	Outline { headings, list_items, fences }
}

pub fn chunk_name(markdown: &str) -> String {
	for line in markdown.lines() {
		let trimmed = line.trim_start();
		if heading_level(trimmed).is_some() {
			return format!("\"{}\"", trimmed.trim_start_matches('#').trim());
		}
	}
	"a readme section with no heading".to_string()
}

fn heading_level(line: &str) -> Option<usize> {
	let hashes = line.bytes().take_while(|b| *b == b'#').count();
	(hashes > 0 && hashes <= MAX_HEADING_LEVEL && line.as_bytes().get(hashes) == Some(&b' ')).then_some(hashes)
}

/// Length alone is the wrong bound: a section well under the character limit, with several
/// headings, came back with most of them missing on every attempt. Sections are self-contained, so
/// a request loses nothing by being small.
const MAX_HEADINGS_PER_CHUNK: usize = 1;

fn too_big(markdown: &str, limit: usize) -> bool {
	markdown.len() > limit || outline(markdown).headings.len() > MAX_HEADINGS_PER_CHUNK
}

fn split_oversized(section: String, limit: usize, level: usize) -> Vec<String> {
	if !too_big(&section, limit) || level > MAX_HEADING_LEVEL {
		return vec![section];
	}
	split_at_headings(&section, level).into_iter().flat_map(|piece| split_oversized(piece, limit, level + 1)).collect()
}

/// Splits Markdown into chunks within `limit` characters and [`MAX_HEADINGS_PER_CHUNK`].
///
/// Breaks only at headings, so that the model never sees a half-open construct. A piece with no
/// heading left to cut at stays whole.
pub fn split_markdown(markdown: &str, limit: usize) -> Vec<String> {
	let sections = split_sections(markdown).into_iter().flat_map(|section| split_oversized(section, limit, 3));
	let mut chunks: Vec<String> = Vec::new();
	for section in sections {
		match chunks.last_mut() {
			Some(last) if !too_big(&format!("{last}\n\n{section}"), limit) => {
				last.push_str("\n\n");
				last.push_str(&section);
			}
			_ => chunks.push(section),
		}
	}
	chunks
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_section_with_several_headings_is_cut_even_though_it_fits() {
		let doc = "## Shortcuts\n\n### File\n\n* a\n\n### Go\n\n* b\n\n### Tools\n\n* c\n\n### Help\n\n* d\n";
		let chunks = split_markdown(doc, 10_000);
		assert_eq!(chunks.len(), 5, "got {chunks:?}");
		for chunk in &chunks {
			assert!(outline(chunk).headings.len() <= MAX_HEADINGS_PER_CHUNK, "{chunk:?}");
		}
	}

	#[test]
	fn a_translation_that_kept_every_heading_and_bullet_passes() {
		let source = "## One\n\n* a\n* b\n\n### Two\n\nText.\n";
		let translated = "## Eins\n\n* a\n* b\n\n### Zwei\n\nText.\n";
		assert_eq!(structure_mismatch(source, translated), None);
	}

	#[test]
	fn a_translation_missing_a_section_is_rejected() {
		let source = "## One\n\nText.\n\n## Two\n\nText.\n\n## Three\n\nText.\n";
		let translated = "## Eins\n\nText.\n\n## Drei\n\nText.\n";
		assert!(structure_mismatch(source, translated).is_some_and(|why| why.contains("headings")));
	}

	#[test]
	fn a_translation_missing_list_items_is_rejected() {
		let source = "## Formats\n\n* EPUB\n* PDF\n* RTF\n";
		let translated = "## Formate\n\n* EPUB\n* PDF\n";
		assert!(structure_mismatch(source, translated).is_some_and(|why| why.contains("list items")));
	}

	#[test]
	fn an_empty_translation_is_rejected() {
		assert!(structure_mismatch("## One\n\nText.\n", "   ").is_some());
	}

	/// A `#` and a `*` inside a fenced block are content, not structure.
	#[test]
	fn fenced_blocks_do_not_count_as_headings_or_bullets() {
		let source = "## One\n\n```\n# not a heading\n* not a bullet\n```\n";
		let translated = "## Eins\n\n```\n# not a heading\n* not a bullet\n```\n";
		assert_eq!(structure_mismatch(source, translated), None);
	}

	#[test]
	fn a_hash_without_a_space_is_not_a_heading() {
		let source = "## One\n\nIssue #42 is fixed.\n";
		let translated = "## Eins\n\nProblem #42 ist behoben.\n";
		assert_eq!(structure_mismatch(source, translated), None);
	}

	#[test]
	fn markdown_splits_on_section_headings() {
		let doc = "# Title\n\nIntro.\n\n## One\n\nBody one.\n\n## Two\n\nBody two.\n";
		let chunks = split_markdown(doc, 30);
		assert_eq!(chunks.len(), 3, "each section should be its own chunk at this limit");
		assert!(chunks[1].starts_with("## One"));
		assert!(chunks[2].starts_with("## Two"));
	}

	#[test]
	fn each_section_is_its_own_chunk_however_large_the_limit() {
		let doc = "# Title\n\nIntro.\n\n## One\n\nBody one.\n\n## Two\n\nBody two.\n";
		let chunks = split_markdown(doc, 10_000);
		assert_eq!(chunks.len(), 3, "got {chunks:?}");
		assert_eq!(chunks.join("\n\n").trim(), doc.trim(), "no text lost or duplicated");
	}

	#[test]
	fn an_oversized_section_is_cut_at_its_deeper_headings() {
		let doc = "## Changelog\n\n### Version 2\n\n* Two.\n\n### Version 1\n\n#### Added\n\n* One.\n\n#### Fixed\n\n* Won.\n";
		let chunks = split_markdown(doc, 30);
		assert_eq!(chunks.len(), 5, "got {chunks:?}");
		assert!(chunks[0].starts_with("## Changelog"));
		assert!(chunks[1].starts_with("### Version 2"));
		assert!(chunks[2].starts_with("### Version 1"));
		assert!(chunks[3].starts_with("#### Added"));
		assert!(chunks[4].starts_with("#### Fixed"));
		assert_eq!(chunks.join("\n\n"), doc.trim_end());
	}

	#[test]
	fn a_translated_key_name_is_put_back() {
		let source = "Press `Alt+Left` to go back, or `Ctrl+Space` to play.";
		let translated = "Appuyez sur `Alt+Gauche` pour revenir, ou `Ctrl+Espace` pour lire.";
		assert_eq!(
			restore_code_spans(source, translated),
			"Appuyez sur `Alt+Left` pour revenir, ou `Ctrl+Space` pour lire."
		);
	}

	#[test]
	fn prose_around_a_restored_span_is_left_alone() {
		let source = "The `readme.md` file.";
		let translated = "Het `readme.md` bestand.";
		assert_eq!(restore_code_spans(source, translated), translated);
	}

	// A changed count means the nth span in the translation is no longer the nth in the
	// source, so restoring by position would drop text into the wrong place.
	#[test]
	fn a_mismatched_span_count_leaves_the_translation_untouched() {
		let source = "Press `Alt+Left` then `Ctrl+C`.";
		let translated = "Appuyez sur `Alt+Gauche`.";
		assert_eq!(restore_code_spans(source, translated), translated);
	}

	#[test]
	fn code_spans_do_not_run_past_a_newline() {
		// A stray unmatched backtick must not swallow the rest of the document.
		let spans = code_spans("a ` stray\nand `real` one");
		assert_eq!(spans.len(), 1);
		assert_eq!(spans[0].1, "real");
	}

	#[test]
	fn an_empty_span_is_not_a_span() {
		assert_eq!(code_spans("nothing `` here"), [] as [(std::ops::Range<usize>, &str); 0]);
	}

	#[test]
	fn restoring_handles_multibyte_text_around_the_spans() {
		let source = "Press `Alt+Left` now.";
		let translated = "Нажмите `Alt+Влево` сейчас.";
		assert_eq!(restore_code_spans(source, translated), "Нажмите `Alt+Left` сейчас.");
	}

	#[test]
	fn markdown_round_trips_when_nothing_needs_splitting() {
		let doc = "# Title\n\nIntro.\n\n## One\n\nBody one.";
		assert_eq!(split_markdown(doc, 10_000).join("\n\n"), doc);
	}
}
