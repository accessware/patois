//! Machine translation of gettext catalogs and Markdown readmes through the Claude API, for apps
//! that use [patois](https://docs.rs/patois).
//!
//! [`translate`] syncs each `.po` file with the `.pot` through `msgmerge`, and then asks Claude to
//! fill the blank entries. Each request includes the `TRANSLATORS:` note of each string, and each
//! answer goes through [`patois_build::check`], so a translation that loses a placeholder, an
//! accelerator, or a shortcut is never written. The function can also translate a readme into
//! `readme-<lang>.md` files, one section at a time.
//!
//! Call it from an `xtask` after you regenerate the `.pot`. It needs `msgmerge` from gettext and,
//! unless it's a dry run, an `ANTHROPIC_API_KEY`. To use a model other than the default, set
//! `PATOIS_TRANSLATE_MODEL`.
//!
//! Two optional files in the `.po` directory change what it does:
//!
//! - `human-maintained-locales.txt`: locales that people translate, one per line. The function
//!   skips them.
//! - `style/<lang>.md`: conventions for one language, which are added to its instructions.

use std::{
	collections::{HashMap, HashSet},
	env,
	error::Error,
	fs,
	path::{Path, PathBuf},
	process::{self, Command},
};

use patois_build::{
	check::is_damaged,
	po::{PoDocument, Translation},
};

mod claude;
mod markdown;
mod prompts;
mod readme;
mod terms;
mod update;

/// The app that the model translates for.
#[derive(Debug, Clone, Default)]
pub struct App {
	/// The name of the app, such as `Paperback`.
	pub name: String,
	/// A description that completes "the user interface of Paperback, …", such as `a desktop ebook
	/// and document reader used heavily with screen readers`.
	pub description: String,
	/// Names to leave untranslated besides the app's own, such as `EPUB` and `PDF`. File
	/// extensions and URLs are always left alone.
	pub proper_nouns: Vec<String>,
}

/// Where a project keeps its translations. The paths other than `root` are relative to `root`.
#[derive(Debug, Clone)]
pub struct Project<'a> {
	/// The root directory of the project.
	pub root: &'a Path,
	/// The directory with the `.po` files, such as `po`.
	pub po_dir: &'a Path,
	/// The template, such as `po/myapp.pot`.
	pub pot: &'a Path,
	/// The English readme to translate into `<stem>-<lang>.md` next to it, such as `doc/readme.md`.
	pub readme: Option<&'a Path>,
	/// The app that the strings belong to.
	pub app: App,
}

/// Options for [`translate`].
#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools, reason = "these are independent switches")]
pub struct Options {
	/// Reports what would be translated, without calling the API or changing a file.
	pub dry_run: bool,
	/// Also translates again the entries whose translation lost or added a placeholder, an
	/// accelerator, or a shortcut.
	pub repair: bool,
	/// Also translates again the machine translations that are shared with a string that says
	/// something different. `msgmerge` can copy these in. Use this option once, not on every run.
	pub repair_copies: bool,
}

/// Translates every `.po` file of `project` that isn't human-maintained, then its readme.
///
/// A file is written only if its content changed, not if `msgmerge` updated only the timestamps in
/// its header.
///
/// # Errors
///
/// Returns an error if `ANTHROPIC_API_KEY` isn't set outside a dry run, if a file can't be read or
/// written, if an API request fails, or if a readme section comes back incomplete.
pub fn translate(project: &Project, options: Options) -> Result<(), Box<dyn Error>> {
	let client = if options.dry_run {
		None
	} else {
		let api_key = env::var("ANTHROPIC_API_KEY").map_err(|_| "ANTHROPIC_API_KEY environment variable is not set")?;
		// A secret that exists but is empty passes `env::var`, then fails at the API with an
		// error that reads like a wrong key rather than a missing one.
		if api_key.trim().is_empty() {
			return Err("ANTHROPIC_API_KEY is set but empty".into());
		}
		let client = claude::ClaudeClient::new(api_key, project.app.clone());
		println!("translating with {}", client.model());
		Some(client)
	};
	let pot_path = project.root.join(project.pot);
	let context = translator_comments(&fs::read_to_string(&pot_path)?);
	let po_dir = project.root.join(project.po_dir);
	let mut po_files: Vec<PathBuf> = fs::read_dir(&po_dir)?
		.filter_map(Result::ok)
		.map(|e| e.path())
		.filter(|p| p.extension().and_then(|e| e.to_str()) == Some("po"))
		.collect();
	po_files.sort();
	let human_maintained = load_human_maintained_locales(project);
	let mut auto_langs = Vec::new();
	for po_path in &po_files {
		let lang = po_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
		if human_maintained.contains(lang) {
			println!("{lang}: human-maintained, skipping");
			continue;
		}
		auto_langs.push(lang.to_string());
		translate_one(project, po_path, &pot_path, options, client.as_ref(), &context)?;
	}
	readme::sync_readmes(project, &auto_langs, client.as_ref(), options.dry_run)
}

fn translator_comments(pot: &str) -> HashMap<String, String> {
	PoDocument::parse(pot)
		.entries
		.into_iter()
		.filter_map(|entry| Some((entry.msgid, entry.comment?)))
		.filter(|(msgid, _)| !msgid.is_empty())
		.collect()
}

fn load_human_maintained_locales(project: &Project) -> HashSet<String> {
	let path = project.root.join(project.po_dir).join("human-maintained-locales.txt");
	fs::read_to_string(path).map(|content| parse_human_maintained_locales(&content)).unwrap_or_default()
}

fn parse_human_maintained_locales(content: &str) -> HashSet<String> {
	content
		.lines()
		.map(|line| line.split('#').next().unwrap_or("").trim())
		.filter(|line| !line.is_empty())
		.map(str::to_string)
		.collect()
}

fn load_style_note(project: &Project, lang: &str) -> Option<String> {
	let path = project.root.join(project.po_dir).join("style").join(format!("{lang}.md"));
	parse_style_note(&fs::read_to_string(path).ok()?)
}

fn parse_style_note(content: &str) -> Option<String> {
	let note = content.trim();
	(!note.is_empty()).then(|| note.to_string())
}

fn style_note_suffix(project: &Project, lang: &str, present: bool) -> String {
	if present {
		format!(
			", with {}",
			project.po_dir.join("style").join(format!("{lang}.md")).to_string_lossy().replace('\\', "/")
		)
	} else {
		String::new()
	}
}

type Applied = Vec<(usize, Translation)>;

fn translate_singulars(
	client: &claude::ClaudeClient,
	target: &claude::Target,
	candidates: &[(usize, String)],
	context: &HashMap<String, String>,
) -> Result<(Applied, usize), Box<dyn Error>> {
	if candidates.is_empty() {
		return Ok((Vec::new(), 0));
	}
	let phrases: Vec<claude::Phrase> = candidates
		.iter()
		.map(|(_, text)| claude::Phrase { source: text.clone(), context: context.get(text).cloned() })
		.collect();
	let annotated = phrases.iter().filter(|p| p.context.is_some()).count();
	let results = client.translate_phrases(&phrases, target)?;
	let applied = candidates
		.iter()
		.map(|(i, _)| *i)
		.zip(results)
		.filter_map(|(i, result)| result.map(|text| (i, Translation::Singular(text))))
		.collect();
	Ok((applied, annotated))
}

fn translate_plurals(
	client: &claude::ClaudeClient,
	target: &claude::Target,
	candidates: &[(usize, String, String)],
	context: &HashMap<String, String>,
	nplurals: usize,
	rule: &str,
) -> Result<Applied, Box<dyn Error>> {
	if candidates.is_empty() {
		return Ok(Vec::new());
	}
	let phrases: Vec<claude::PluralPhrase> = candidates
		.iter()
		.map(|(_, singular, plural)| claude::PluralPhrase {
			singular: singular.clone(),
			plural: plural.clone(),
			// The note is above the singular, which is the msgid.
			context: context.get(singular).cloned(),
		})
		.collect();
	let results = client.translate_plurals(&phrases, target, nplurals, rule)?;
	Ok(candidates
		.iter()
		.map(|(i, _, _)| *i)
		.zip(results)
		.filter_map(|(i, result)| result.map(|forms| (i, Translation::Plural(forms))))
		.collect())
}

fn plural_forms(content: &str) -> Option<(usize, String)> {
	let nplurals = PoDocument::parse(content).nplurals()?;
	let line = content.lines().map(str::trim).find(|l| l.contains("Plural-Forms:"))?;
	let rule = line.trim_start_matches('"').trim_end_matches("\\n\"").trim().to_string();
	Some((nplurals, rule))
}

/// Adds the entries whose translation is damaged to the candidates, and returns how many.
fn add_damaged_entries(
	doc: &PoDocument,
	candidates: &mut Vec<(usize, String)>,
	plurals: &mut Vec<(usize, String, String)>,
) -> usize {
	let already: HashSet<usize> = candidates.iter().map(|(i, _)| *i).collect();
	let already_plural: HashSet<usize> = plurals.iter().map(|(i, _, _)| *i).collect();
	let mut count = 0;
	for (i, entry) in doc.entries.iter().enumerate() {
		match entry.msgid_plural.as_deref() {
			// The forms are translated together, so one damaged form means all of them.
			Some(plural) => {
				if !already_plural.contains(&i) && entry.msgstr_plural.iter().any(|form| is_damaged(plural, form)) {
					plurals.push((i, entry.msgid.clone(), plural.to_string()));
					count += 1;
				}
			}
			None => {
				if !already.contains(&i) && is_damaged(&entry.msgid, &entry.msgstr) {
					candidates.push((i, entry.msgid.clone()));
					count += 1;
				}
			}
		}
	}
	count
}

/// Adds the fuzzy entries whose translation is shared with an entry whose English says something
/// different, and returns how many.
///
/// Only fuzzy entries qualify, because a person wrote the others, and a shared translation isn't
/// enough reason to replace their work.
fn add_copied_entries(doc: &PoDocument, candidates: &mut Vec<(usize, String)>) -> usize {
	let already: HashSet<usize> = candidates.iter().map(|(i, _)| *i).collect();
	let mut sources: HashMap<&str, HashSet<String>> = HashMap::new();
	for entry in &doc.entries {
		if entry.msgid_plural.is_none() && !entry.msgid.is_empty() && !entry.msgstr.is_empty() {
			sources.entry(entry.msgstr.as_str()).or_default().insert(same_meaning_key(&entry.msgid));
		}
	}
	let mut count = 0;
	for (i, entry) in doc.entries.iter().enumerate() {
		let shared = sources.get(entry.msgstr.as_str()).is_some_and(|keys| keys.len() > 1);
		if entry.is_fuzzy && entry.msgid_plural.is_none() && shared && !already.contains(&i) {
			candidates.push((i, entry.msgid.clone()));
			count += 1;
		}
	}
	count
}

/// Returns the words of a string, without accelerators, case, spaces, and end punctuation. Two
/// strings with the same key can share a translation.
fn same_meaning_key(msgid: &str) -> String {
	msgid
		.chars()
		.filter(|c| !matches!(c, '&' | '.' | ':' | '…' | '!' | '?') && !c.is_whitespace())
		.flat_map(char::to_lowercase)
		.collect()
}

/// Fills each blank entry from an obsolete entry with the same words, and returns the new file and
/// the number of entries filled.
///
/// A string often changes only where its accelerator sits. `--no-fuzzy-matching` retires the old
/// entry and leaves the new one blank, which would discard a translation that a person wrote. The
/// restored translation keeps its own accelerator, which its translator chose for that language.
fn restore_obsolete(content: String) -> (String, usize) {
	let mut doc = PoDocument::parse(&content);
	let mut found: HashMap<String, Vec<(String, bool)>> = HashMap::new();
	// A person's translation wins over a machine's.
	let mut obsolete: Vec<_> = doc.obsolete.iter().filter(|entry| !entry.msgstr.is_empty()).collect();
	obsolete.sort_by_key(|entry| entry.is_fuzzy);
	for entry in obsolete {
		found.entry(same_meaning_key(&entry.msgid)).or_default().push((entry.msgstr.clone(), entry.is_fuzzy));
	}
	let restored: Vec<(usize, Translation, bool)> = doc
		.needs_translation()
		.filter(|(i, _)| doc.entries[*i].msgstr.is_empty())
		.filter_map(|(i, msgid)| {
			let (text, fuzzy) =
				found.get(&same_meaning_key(msgid))?.iter().find(|(text, _)| !is_damaged(msgid, text))?;
			Some((i, Translation::Singular(text.clone()), *fuzzy))
		})
		.collect();
	if restored.is_empty() {
		return (content, 0);
	}
	let flagged: Vec<(usize, &Translation, bool)> = restored.iter().map(|(i, t, fuzzy)| (*i, t, *fuzzy)).collect();
	doc.apply_flagged(&flagged);
	(doc.render(), restored.len())
}

fn translate_one(
	project: &Project,
	po_path: &Path,
	pot_path: &Path,
	options: Options,
	client: Option<&claude::ClaudeClient>,
	context: &HashMap<String, String>,
) -> Result<(), Box<dyn Error>> {
	let lang = po_path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
	let style = load_style_note(project, &lang);
	let original = fs::read_to_string(po_path)?;
	// msgmerge works on a copy, so that the real file changes only if its content does.
	let tmp = env::temp_dir().join(format!("patois-translate-{lang}-{}.po", process::id()));
	fs::write(&tmp, &original)?;
	// Without `--no-fuzzy-matching`, msgmerge gives a new string the translation of the most
	// similar old one, and fuzzy entries reach the app, so users would see that guess.
	let merged_ok = Command::new("msgmerge")
		.args(["--update", "--backup=none", "--no-wrap", "--no-fuzzy-matching"])
		.arg(&tmp)
		.arg(pot_path)
		.status()
		.is_ok_and(|s| s.success());
	if !merged_ok {
		eprintln!("warning: msgmerge failed for {lang}, leaving it untouched this run");
		let _ = fs::remove_file(&tmp);
		return Ok(());
	}
	let merged = fs::read_to_string(&tmp)?;
	let _ = fs::remove_file(&tmp);
	let (merged, restored) = restore_obsolete(merged);
	if restored > 0 {
		let verb = if client.is_some() { "restored" } else { "would restore" };
		println!("{lang}: {verb} {restored} entries from obsolete ones with the same words");
	}
	let mut doc = PoDocument::parse(&merged);
	let mut candidates: Vec<(usize, String)> = doc.needs_translation().map(|(i, m)| (i, m.to_string())).collect();
	let mut plurals: Vec<(usize, String, String)> =
		doc.needs_plural_translation().map(|(i, s, p)| (i, s.to_string(), p.to_string())).collect();
	let mut repaired = if options.repair { add_damaged_entries(&doc, &mut candidates, &mut plurals) } else { 0 };
	if options.repair_copies {
		repaired += add_copied_entries(&doc, &mut candidates);
	}
	let total = candidates.len() + plurals.len();
	let Some(client) = client else {
		let style_note = style_note_suffix(project, &lang, style.is_some());
		if total == 0 {
			println!("{lang}: fully translated, nothing to do{style_note}");
		} else {
			let plural_note = if plurals.is_empty() { String::new() } else { format!(", {} plural", plurals.len()) };
			let repair_note = if repaired > 0 { format!(" ({repaired} damaged)") } else { String::new() };
			println!("{lang}: {total} entries would be translated{plural_note}{repair_note}{style_note}");
		}
		return Ok(());
	};
	let final_content = if total == 0 {
		merged
	} else {
		let language = patois::language_name(&lang);
		let target = claude::Target { language, style: style.as_deref() };
		let (mut applied, annotated) = translate_singulars(client, &target, &candidates, context)?;
		let plural_done = match plural_forms(&merged) {
			// Without the header there's no way to know how many forms to ask for, and a wrong
			// count makes a file that gettext accepts without a warning.
			None if !plurals.is_empty() => {
				eprintln!("warning: {lang} has no usable Plural-Forms header, leaving its plural entries");
				0
			}
			None => 0,
			Some((nplurals, rule)) => {
				let translated = translate_plurals(client, &target, &plurals, context, nplurals, &rule)?;
				let done = translated.len();
				applied.extend(translated);
				done
			}
		};
		let singular_done = applied.len() - plural_done;
		let skipped = total - applied.len();
		doc.apply(&applied);
		print!("{lang} ({language}): translated {singular_done} entries");
		if plural_done > 0 {
			print!(", {plural_done} plural");
		}
		if repaired > 0 {
			print!(", {repaired} of them repaired");
		}
		if annotated > 0 {
			print!(", {annotated} with translator notes");
		}
		if skipped > 0 {
			print!(", skipped {skipped} (failed a placeholder, accelerator, or shortcut check, will retry next run)");
		}
		println!();
		doc.render()
	};
	if content_without_volatile_headers(&original) != content_without_volatile_headers(&final_content) {
		fs::write(po_path, final_content)?;
	}
	Ok(())
}

// `msgmerge` changes these two header lines on every run.
fn content_without_volatile_headers(content: &str) -> String {
	content
		.lines()
		.filter(|line| {
			let t = line.trim();
			!(t.starts_with("\"POT-Creation-Date:") || t.starts_with("\"PO-Revision-Date:"))
		})
		.collect::<Vec<_>>()
		.join("\n")
}

#[cfg(test)]
mod tests {
	use std::fmt::Write as _;

	use super::*;

	#[test]
	fn timestamp_only_changes_are_ignored() {
		let a = "msgid \"\"\nmsgstr \"\"\n\"POT-Creation-Date: 2026-01-01 00:00+0000\\n\"\n\"PO-Revision-Date: 2026-01-01 00:00+0000\\n\"\n\"Language: de\\n\"\n";
		let b = "msgid \"\"\nmsgstr \"\"\n\"POT-Creation-Date: 2026-06-01 12:00+0000\\n\"\n\"PO-Revision-Date: 2026-06-01 12:00+0000\\n\"\n\"Language: de\\n\"\n";
		assert_eq!(content_without_volatile_headers(a), content_without_volatile_headers(b));
	}

	#[test]
	fn real_content_changes_are_detected() {
		let a = "msgid \"Cancel\"\nmsgstr \"\"\n";
		let b = "msgid \"Cancel\"\nmsgstr \"Abbrechen\"\n";
		assert_ne!(content_without_volatile_headers(a), content_without_volatile_headers(b));
	}

	#[test]
	fn human_maintained_locales_ignores_comments_and_blank_lines() {
		let content = "# comment\n\nfi   # Jani Kinnunen\n\n  sr  \n";
		assert_eq!(parse_human_maintained_locales(content), HashSet::from(["fi".to_string(), "sr".to_string()]));
		assert!(parse_human_maintained_locales("# nothing here yet\n").is_empty());
	}

	#[test]
	fn a_style_note_is_trimmed() {
		let content = "\n\n# Dutch\n\nAddress the reader as \"je\".\n\n\n";
		assert_eq!(parse_style_note(content), Some("# Dutch\n\nAddress the reader as \"je\".".to_string()));
		assert_eq!(parse_style_note("  \n\n"), None);
	}

	#[test]
	fn the_dry_run_line_names_the_style_note_it_would_use() {
		let project = Project {
			root: Path::new("."),
			po_dir: Path::new("po"),
			pot: Path::new("po/app.pot"),
			readme: None,
			app: App::default(),
		};
		assert_eq!(style_note_suffix(&project, "nl", true), ", with po/style/nl.md");
		assert_eq!(style_note_suffix(&project, "nl", false), "");
	}

	#[test]
	fn a_translator_note_attaches_to_the_msgid_below_it_only() {
		let pot = "#. TRANSLATORS: about Ready\nmsgid \"Ready\"\nmsgstr \"\"\n\nmsgid \"Cancel\"\nmsgstr \"\"\n";
		let notes = translator_comments(pot);
		assert_eq!(notes.get("Ready").map(String::as_str), Some("about Ready"));
		assert!(!notes.contains_key("Cancel"));
	}

	#[test]
	fn keys_are_unescaped_and_joined_across_continuation_lines() {
		let pot =
			"#. TRANSLATORS: a two-line prompt\nmsgid \"\"\n\"No parser for {}.\\n\"\n\"Open it how?\"\nmsgstr \"\"\n";
		let notes = translator_comments(pot);
		assert_eq!(notes.get("No parser for {}.\nOpen it how?").map(String::as_str), Some("a two-line prompt"));
	}

	#[test]
	fn the_header_entry_never_takes_a_note() {
		let pot = "#. TRANSLATORS: stray\nmsgid \"\"\nmsgstr \"\"\n\nmsgid \"Ready\"\nmsgstr \"\"\n";
		assert!(translator_comments(pot).is_empty());
	}

	#[test]
	fn plural_forms_reads_the_count_and_the_rule() {
		let po = "msgid \"\"\nmsgstr \"\"\n\"Plural-Forms: nplurals=3; plural=(n%10==1 && n%100!=11 ? 0 : 1);\\n\"\n";
		let (nplurals, rule) = plural_forms(po).unwrap();
		assert_eq!(nplurals, 3);
		assert!(rule.starts_with("Plural-Forms: nplurals=3;"), "got: {rule}");
		assert!(!rule.ends_with("\\n\""), "got: {rule}");
		assert!(plural_forms("msgid \"\"\nmsgstr \"\"\n").is_none());
	}

	fn catalog(entries: &[(&str, &str, bool)]) -> PoDocument {
		let mut text = String::from("msgid \"\"\nmsgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n");
		for (msgid, msgstr, fuzzy) in entries {
			text.push('\n');
			if *fuzzy {
				text.push_str("#, fuzzy\n");
			}
			let _ = writeln!(text, "msgid \"{msgid}\"\nmsgstr \"{msgstr}\"");
		}
		PoDocument::parse(&text)
	}

	fn copied(doc: &PoDocument) -> Vec<String> {
		let mut candidates = Vec::new();
		add_copied_entries(doc, &mut candidates);
		candidates.into_iter().map(|(_, msgid)| msgid).collect()
	}

	#[test]
	fn a_translation_shared_by_strings_that_say_different_things_is_a_copy() {
		let doc = catalog(&[("1 minute", "1 minuto", true), ("10 minutes", "1 minuto", true)]);
		assert_eq!(vec!["1 minute", "10 minutes"], copied(&doc));
	}

	#[test]
	fn the_same_words_sharing_a_translation_are_not_a_copy() {
		let doc = catalog(&[
			("&Close", "Cerrar", true),
			("Close", "Cerrar", true),
			("Open...", "Abrir", true),
			("Open", "Abrir", true),
		]);
		assert!(copied(&doc).is_empty());
	}

	#[test]
	fn a_string_whose_accelerator_moved_keeps_its_translation() {
		let po = "msgid \"\"\nmsgstr \"Content-Type: text/plain; charset=UTF-8\\n\"\n\n#. TRANSLATORS: menu item\nmsgid \"&Toggle Bookmark\"\nmsgstr \"\"\n\nmsgid \"Go to li&ne...\"\nmsgstr \"\"\n\nmsgid \"&View\"\nmsgstr \"\"\n\n#~ msgid \"Toggle &Bookmark\"\n#~ msgstr \"&Lesezeichen umschalten\"\n\n#, fuzzy\n#~ msgid \"Go to &line...\"\n#~ msgstr \"Gehe zu &Zeile...\"\n";
		let (restored, count) = restore_obsolete(po.to_string());
		assert_eq!(count, 2);
		assert!(
			restored.contains(
				"#. TRANSLATORS: menu item\nmsgid \"&Toggle Bookmark\"\nmsgstr \"&Lesezeichen umschalten\"\n"
			),
			"{restored}"
		);
		assert!(restored.contains("#, fuzzy\nmsgid \"Go to li&ne...\"\nmsgstr \"Gehe zu &Zeile...\"\n"), "{restored}");
		assert!(restored.contains("msgid \"&View\"\nmsgstr \"\"\n"), "{restored}");
	}

	#[test]
	fn a_person_s_obsolete_translation_wins_over_a_machine_s() {
		let po = "msgid \"&Links\"\nmsgstr \"\"\n\n#, fuzzy\n#~ msgid \"Lin&ks\"\n#~ msgstr \"&Verknüpfungen\"\n\n#~ msgid \"Links\"\n#~ msgstr \"Links\"\n\n#~ msgid \"L&inks\"\n#~ msgstr \"&Links\"\n";
		let (restored, _) = restore_obsolete(po.to_string());
		assert!(restored.starts_with("msgid \"&Links\"\nmsgstr \"&Links\"\n"), "{restored}");
	}

	#[test]
	fn an_obsolete_translation_that_fails_the_check_is_not_restored() {
		let po = "msgid \"&Open %s\"\nmsgstr \"\"\n\n#~ msgid \"Open %s\"\n#~ msgstr \"%s öffnen\"\n";
		assert_eq!(restore_obsolete(po.to_string()).1, 0);
	}

	#[test]
	fn a_string_with_different_words_is_not_restored() {
		let po = "msgid \"Export\"\nmsgstr \"\"\n\n#~ msgid \"I&mport/Export\"\n#~ msgstr \"I&mport/Export\"\n";
		assert_eq!(restore_obsolete(po.to_string()).1, 0);
	}

	#[test]
	fn a_human_translation_is_never_selected() {
		let doc = catalog(&[("Match Case", "Mayusculas", false), ("Batch OCR", "Mayusculas", true)]);
		assert_eq!(vec!["Batch OCR"], copied(&doc));
	}
}
