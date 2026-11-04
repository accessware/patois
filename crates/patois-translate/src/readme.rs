use std::{
	collections::{HashMap, hash_map::DefaultHasher},
	error::Error,
	fmt::Write as _,
	fs,
	hash::{Hash, Hasher},
	path::Path,
	process::Command,
};

use patois::language_name;

use crate::{
	Project,
	claude::{ClaudeClient, Guidance, Target},
	load_style_note,
	markdown::{self, split_sections},
	style_note_suffix,
	terms::Terms,
	update,
};

const MARKER_START: &str = "<!-- machine-translated from ";
const HASH_TAG: &str = " (source-hash: ";
const MARKER_SUFFIX: &str = "); please review and edit as needed -->";
const SECTIONS_TAG: &str = "; sections: ";
const HISTORY_LIMIT: usize = 200;

/// Translates the readme into `<stem>-<lang>.md` for each language in `langs`.
///
/// Only the sections that changed go to the model, so a one-line edit costs one request and leaves
/// the other sections as they were. A file written by hand goes through the same steps, because
/// nothing can tell how stale it has become.
pub fn sync_readmes(
	project: &Project,
	langs: &[String],
	client: Option<&ClaudeClient>,
	dry_run: bool,
) -> Result<(), Box<dyn Error>> {
	let Some(readme) = project.readme else {
		return Ok(());
	};
	let source_path = project.root.join(readme);
	let doc_dir = source_path.parent().unwrap_or(project.root).to_path_buf();
	let stem = readme.file_stem().and_then(|stem| stem.to_str()).unwrap_or("readme");
	let label = readme.to_string_lossy().replace('\\', "/");
	let Ok(source_md) = fs::read_to_string(&source_path) else {
		return Ok(());
	};
	let hash = source_hash(&source_md);
	let sections = split_sections(&source_md);
	let hashes: Vec<String> = sections.iter().map(|section| section_hash(section)).collect();
	let mut failed: Vec<String> = Vec::new();
	let mut previous: HashMap<String, Option<Vec<String>>> = HashMap::new();
	for lang in langs {
		let target_path = doc_dir.join(format!("{stem}-{lang}.md"));
		let existing = fs::read_to_string(&target_path).ok();
		if !needs_translation(existing.as_deref(), &hash) {
			continue;
		}
		let reusable = existing.as_deref().and_then(|content| reusable_sections(content, &hashes));
		let to_translate =
			reusable.as_ref().map_or(sections.len(), |r| r.iter().filter(|s| s.kept().is_none()).count());
		let was = existing.as_deref().and_then(existing_marker_hash).and_then(|was_hash| {
			previous
				.entry(was_hash.to_string())
				.or_insert_with(|| previous_source(project.root, readme, was_hash).map(|md| split_sections(&md)))
				.clone()
		});
		let style = load_style_note(project, lang);
		if dry_run {
			let style_note = style_note_suffix(project, lang, style.is_some());
			match &reusable {
				Some(reusable) => println!(
					"{stem}-{lang}.md: would translate {to_translate} of {} sections{}{style_note}",
					sections.len(),
					update_summary(&sections, reusable, was.as_deref())
				),
				None => println!("{stem}-{lang}.md: would be translated in full{style_note}"),
			}
			continue;
		}
		let Some(client) = client else { unreachable!("client is always Some outside a dry run") };
		let language = language_name(lang);
		let target = Target { language, style: style.as_deref() };
		let catalog =
			fs::read_to_string(project.root.join(project.po_dir).join(format!("{lang}.po"))).unwrap_or_default();
		let terms = Terms::from_catalog(&catalog);
		// A run that gives up on the first bad language would leave every later language stale for
		// as long as that one kept failing.
		match translate_document(client, &sections, reusable, was.as_deref(), &terms, &target) {
			Ok(translated_md) => {
				fs::write(
					&target_path,
					format!("{}\n\n{}\n", marker_line(&label, &hash, &hashes), translated_md.trim_end()),
				)?;
				println!("{stem}-{lang}.md ({language}): translated {to_translate} of {} sections", sections.len());
			}
			Err(e) => {
				eprintln!("{stem}-{lang}.md ({language}): not written: {e}");
				failed.push(lang.clone());
			}
		}
	}
	if !failed.is_empty() {
		return Err(format!("the readme came back incomplete for: {}", failed.join(", ")).into());
	}
	Ok(())
}

fn translate_document(
	client: &ClaudeClient,
	sections: &[String],
	reusable: Option<Vec<Existing>>,
	was: Option<&[String]>,
	terms: &Terms,
	target: &Target,
) -> Result<String, Box<dyn Error>> {
	let mut out: Vec<String> = Vec::with_capacity(sections.len());
	// Section by section even with nothing to carry over, so that a section that comes back short
	// is caught against its source.
	let reusable = reusable
		.map_or_else(|| sections.iter().map(|_| None).collect(), |r| r.into_iter().map(Some).collect::<Vec<_>>());
	for (section, existing) in sections.iter().zip(reusable) {
		let section_terms = terms.for_section(section);
		match existing {
			Some(Existing::Kept(text)) => out.push(text),
			Some(Existing::Outdated { translation, was_hash }) => {
				match was.and_then(|was| was.iter().find(|old| section_hash(old) == was_hash)) {
					Some(old) => {
						out.push(client.update_markdown(old, section, &translation, &section_terms, target)?);
					}
					None => out.push(client.translate_markdown(
						section,
						&Guidance { existing: Some(&translation), terms: &section_terms },
						target,
					)?),
				}
			}
			None => out.push(client.translate_markdown(
				section,
				&Guidance { existing: None, terms: &section_terms },
				target,
			)?),
		}
	}
	Ok(out.join("\n\n"))
}

fn update_summary(sections: &[String], reusable: &[Existing], was: Option<&[String]>) -> String {
	let mut lines = 0;
	let mut whole: Vec<String> = Vec::new();
	for (section, existing) in sections.iter().zip(reusable) {
		let Existing::Outdated { translation, was_hash } = existing else { continue };
		let old = was.and_then(|was| was.iter().find(|old| section_hash(old) == *was_hash));
		match old.and_then(|old| update::plan(old, section, translation)) {
			Some(plan) => lines += update::new_line_count(&plan),
			None => whole.push(markdown::chunk_name(section)),
		}
	}
	let mut summary = format!(" ({lines} changed {}", if lines == 1 { "line" } else { "lines" });
	if !whole.is_empty() {
		let _ = write!(summary, "; whole, with the existing text as guidance: {}", whole.join(", "));
	}
	summary.push(')');
	summary
}

fn needs_translation(existing_content: Option<&str>, current_source_hash: &str) -> bool {
	existing_content.and_then(existing_marker_hash).is_none_or(|existing_hash| existing_hash != current_source_hash)
}

#[derive(Debug, PartialEq, Eq)]
enum Existing {
	Kept(String),
	Outdated { translation: String, was_hash: String },
}

impl Existing {
	fn kept(&self) -> Option<&str> {
		match self {
			Self::Kept(text) => Some(text),
			Self::Outdated { .. } => None,
		}
	}
}

/// Returns the existing translation of each source section, or `None` if the sections can't be
/// lined up: the file has no section hashes, or the counts of sections disagree.
///
/// A section added or removed shifts every later section, so pairing by position could attach a
/// heading to the wrong body. Translating the whole document is slower but can't do that.
fn reusable_sections(existing_content: &str, source_hashes: &[String]) -> Option<Vec<Existing>> {
	let stored = existing_marker_section_hashes(existing_content)?;
	if stored.len() != source_hashes.len() {
		return None;
	}
	let body = existing_content.split_once('\n').map_or("", |(_, rest)| rest).trim_start();
	let translated = split_sections(body);
	if translated.len() != stored.len() {
		return None;
	}
	Some(
		stored
			.iter()
			.zip(source_hashes)
			.zip(translated)
			.map(|((was, now), text)| {
				if was == now {
					Existing::Kept(text)
				} else {
					Existing::Outdated { translation: text, was_hash: (*was).to_string() }
				}
			})
			.collect(),
	)
}

/// Finds the English that a translation was made from in the git history, so that [`crate::update`]
/// can tell which lines changed. Returns `None` outside a repository, in a shallow clone, or if no
/// version within [`HISTORY_LIMIT`] matches.
fn previous_source(root: &Path, readme: &Path, hash: &str) -> Option<String> {
	let path = readme.to_string_lossy().replace('\\', "/");
	let log = Command::new("git").current_dir(root).args(["log", "--format=%H", "--", &path]).output().ok()?;
	if !log.status.success() {
		return None;
	}
	String::from_utf8_lossy(&log.stdout).lines().take(HISTORY_LIMIT).find_map(|commit| {
		let shown = Command::new("git").current_dir(root).args(["show", &format!("{commit}:{path}")]).output().ok()?;
		let text = String::from_utf8(shown.stdout).ok().filter(|_| shown.status.success())?;
		// The hash came from the checked-out file, which on Windows can have CRLF where the commit
		// has LF.
		let crlf = text.replace("\r\n", "\n").replace('\n', "\r\n");
		[text, crlf].into_iter().find(|candidate| source_hash(candidate) == hash)
	})
}

fn source_hash(source_md: &str) -> String {
	let mut hasher = DefaultHasher::new();
	source_md.hash(&mut hasher);
	format!("{:016x}", hasher.finish())
}

// Shorter than the document hash, so that the marker stays on one line. A collision costs a section
// that isn't translated again, not a wrong one.
fn section_hash(section: &str) -> String {
	let mut hasher = DefaultHasher::new();
	section.hash(&mut hasher);
	format!("{:08x}", hasher.finish() & 0xFFFF_FFFF)
}

fn marker_line(source: &str, hash: &str, section_hashes: &[String]) -> String {
	format!("{MARKER_START}{source}{HASH_TAG}{hash}{SECTIONS_TAG}{}{MARKER_SUFFIX}", section_hashes.join(","))
}

fn marker_body(content: &str) -> Option<&str> {
	let line = content.lines().next()?.trim().strip_prefix(MARKER_START)?.strip_suffix(MARKER_SUFFIX)?;
	line.split_once(HASH_TAG).map(|(_, body)| body)
}

fn existing_marker_hash(content: &str) -> Option<&str> {
	let marker = marker_body(content)?;
	Some(marker.split_once(SECTIONS_TAG).map_or(marker, |(hash, _)| hash))
}

fn existing_marker_section_hashes(content: &str) -> Option<Vec<&str>> {
	let marker = marker_body(content)?;
	let (_, sections) = marker.split_once(SECTIONS_TAG)?;
	Some(sections.split(',').collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn marker(hash: &str, sections: &[&str]) -> String {
		marker_line("doc/readme.md", hash, &sections.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
	}

	#[test]
	fn no_existing_file_needs_translation() {
		assert!(needs_translation(None, "abc123"));
	}

	#[test]
	fn file_without_marker_needs_translation() {
		assert!(needs_translation(Some("# Paperback\n\nSome text"), "abc123"));
	}

	#[test]
	fn file_with_matching_marker_is_skipped() {
		let hash = source_hash("some source content");
		let content = format!("{}\n\nTranslated text", marker(&hash, &["aaaaaaaa"]));
		assert!(!needs_translation(Some(&content), &hash));
	}

	#[test]
	fn file_with_stale_marker_needs_translation() {
		let content = format!("{}\n\nTranslated text", marker("oldhash0000000", &["aaaaaaaa"]));
		assert!(needs_translation(Some(&content), "newhash0000000"));
	}

	#[test]
	fn source_hash_is_stable_and_content_sensitive() {
		assert_eq!(source_hash("hello"), source_hash("hello"));
		assert_ne!(source_hash("hello"), source_hash("world"));
	}

	#[test]
	fn marker_hash_round_trips_through_the_first_line() {
		let hash = source_hash("readme content here");
		let content = format!("{}\nrest of the file", marker(&hash, &["aaaaaaaa", "bbbbbbbb"]));
		assert_eq!(existing_marker_hash(&content), Some(hash.as_str()));
		assert_eq!(existing_marker_section_hashes(&content), Some(vec!["aaaaaaaa", "bbbbbbbb"]));
	}

	#[test]
	fn only_the_changed_section_is_marked_for_translation() {
		let old_hashes = ["11111111".to_string(), "22222222".to_string(), "33333333".to_string()];
		let content = format!(
			"{}\n\nIntro text\n\n## One\n\nFirst\n\n## Two\n\nSecond",
			marker_line("doc/readme.md", "doc", &old_hashes)
		);
		let now = ["11111111".to_string(), "99999999".to_string(), "33333333".to_string()];
		let reusable = reusable_sections(&content, &now).expect("sections line up");
		assert!(reusable[0].kept().is_some(), "unchanged first section should carry over");
		assert!(reusable[1].kept().is_none(), "changed section should be re-translated");
		assert!(reusable[2].kept().is_some(), "unchanged last section should carry over");
	}

	#[test]
	fn a_real_edit_reuses_every_untouched_section() {
		let before = "# Title

Intro.

## Features

One.

## Changelog

Old entry.";
		let hashes: Vec<String> = split_sections(before).iter().map(|s| section_hash(s)).collect();
		assert_eq!(hashes.len(), 3, "title, features, changelog");
		let translated = format!(
			"{}

# Titre

Intro.

## Fonctions

Une.

## Journal

Ancienne entree.",
			marker_line("doc/readme.md", &source_hash(before), &hashes)
		);
		let after = "# Title

Intro.

## Features

One.

## Changelog

New entry.

Old entry.";
		let now: Vec<String> = split_sections(after).iter().map(|s| section_hash(s)).collect();
		let reusable = reusable_sections(&translated, &now).expect("same shape, so the sections pair up");
		assert_eq!(reusable.iter().filter(|s| s.kept().is_none()).count(), 1, "only the changelog changed");
		assert!(reusable[2].kept().is_none(), "the changelog is the section that changed");
		assert!(reusable[0].kept().unwrap().contains("Titre"), "the title section carries over as translated");
		assert!(reusable[1].kept().unwrap().contains("Fonctions"), "features carries over as translated");
	}

	#[test]
	fn a_file_without_section_hashes_cannot_be_reused() {
		let content = format!("{MARKER_START}doc/readme.md{HASH_TAG}abc{MARKER_SUFFIX}\n\nIntro\n\n## One\n\nFirst");
		assert_eq!(reusable_sections(&content, &["1".to_string(), "2".to_string()]), None);
	}

	#[test]
	fn a_source_that_gained_a_section_cannot_be_reused() {
		let content = format!(
			"{}\n\nIntro\n\n## One\n\nFirst",
			marker_line("doc/readme.md", "doc", &["1".to_string(), "2".to_string()])
		);
		let now = ["1".to_string(), "2".to_string(), "3".to_string()];
		assert_eq!(reusable_sections(&content, &now), None);
	}

	#[test]
	fn a_translation_that_does_not_match_its_own_marker_cannot_be_reused() {
		let content = format!(
			"{}\n\nIntro\n\n## One\n\nFirst\n\n## Two\n\nSecond",
			marker_line("doc/readme.md", "doc", &["1".to_string(), "2".to_string()])
		);
		assert_eq!(reusable_sections(&content, &["1".to_string(), "2".to_string()]), None);
	}
}
