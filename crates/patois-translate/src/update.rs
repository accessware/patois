//! Updates a translated readme section by keeping the translation of each line whose English didn't
//! change, and translating only the rest.
//!
//! The pairing happens here rather than in the model. Asked to keep what didn't change, a model
//! rewords untouched lines. Asked which existing line still fits, it pairs lines with their
//! neighbor's translation below an added line.

use crate::markdown::{restore_code_spans, structure_mismatch};

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
	Kept(String),
	New(String),
}

fn text_lines(markdown: &str) -> Vec<&str> {
	markdown.lines().filter(|line| !line.trim().is_empty()).collect()
}

/// Returns `None` if `was` and `existing` have different numbers of text lines, because then
/// nothing says which translated line belongs to which English one.
pub fn plan(was: &str, now: &str, existing: &str) -> Option<Vec<Line>> {
	let was = text_lines(was);
	let existing = text_lines(existing);
	if was.len() != existing.len() {
		return None;
	}
	let now = text_lines(now);
	let pairs = common_lines(&was, &now);
	let mut kept = pairs.iter().peekable();
	Some(
		now.iter()
			.enumerate()
			.map(|(i, line)| match kept.next_if(|(_, j)| *j == i) {
				Some((w, _)) => Line::Kept(existing[*w].to_string()),
				None => Line::New((*line).to_string()),
			})
			.collect(),
	)
}

/// Finds a longest common subsequence, so that an added or removed line shifts the pairing of later
/// lines rather than breaking it.
fn common_lines(was: &[&str], now: &[&str]) -> Vec<(usize, usize)> {
	let mut lengths = vec![vec![0usize; now.len() + 1]; was.len() + 1];
	for i in (0..was.len()).rev() {
		for j in (0..now.len()).rev() {
			lengths[i][j] =
				if was[i] == now[j] { lengths[i + 1][j + 1] + 1 } else { lengths[i + 1][j].max(lengths[i][j + 1]) };
		}
	}
	let (mut i, mut j) = (0, 0);
	let mut pairs = Vec::new();
	while i < was.len() && j < now.len() {
		if was[i] == now[j] {
			pairs.push((i, j));
			i += 1;
			j += 1;
		} else if lengths[i + 1][j] >= lengths[i][j + 1] {
			i += 1;
		} else {
			j += 1;
		}
	}
	pairs
}

pub fn numbered_new_lines(plan: &[Line]) -> String {
	plan.iter()
		.filter_map(|line| match line {
			Line::New(english) => Some(english.as_str()),
			Line::Kept(_) => None,
		})
		.enumerate()
		.map(|(i, line)| format!("{}: {line}", i + 1))
		.collect::<Vec<_>>()
		.join("\n")
}

pub fn new_line_count(plan: &[Line]) -> usize {
	plan.iter().filter(|line| matches!(line, Line::New(_))).count()
}

/// Builds the updated section. An error says what was wrong with `answer`, for the retry.
pub fn assemble(now: &str, plan: &[Line], answer: &str) -> Result<String, String> {
	let answers: Vec<&str> = answer.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
	let wanted = new_line_count(plan);
	if answers.len() != wanted {
		return Err(format!("{wanted} lines were asked for and {} came back", answers.len()));
	}
	let mut answers = answers.into_iter();
	let mut plan = plan.iter();
	let mut out: Vec<String> = Vec::new();
	for line in now.lines() {
		if line.trim().is_empty() {
			out.push(String::new());
			continue;
		}
		match plan.next() {
			Some(Line::Kept(text)) => out.push(text.clone()),
			Some(Line::New(english)) => out.push(restore_code_spans(english, answers.next().unwrap_or_default())),
			None => return Err("the plan is shorter than the section".to_string()),
		}
	}
	let section = out.join("\n");
	structure_mismatch(now, &section).map_or(Ok(section), Err)
}

#[cfg(test)]
mod tests {
	use super::*;

	const WAS: &str = "### Extra keys\n\n* `Delete` on the tab control: Close the selected tab.\n* `Enter` in the document: Follow a link.\n* `Shift+F10`: Open the context menu.";
	const NOW: &str = "### Extra keys\n\n* `Delete` on the tab control: Close the selected tab.\n* `Ctrl+1` through `Ctrl+9`: Go to the first nine open documents.\n* `Enter` in the document: Follow a link.\n* `Shift+F10`: Open the context menu.";
	const DUTCH: &str = "### Aanvullende toetsen\n\n* `Delete` op het tabbladelement: Het geselecteerde tabblad sluiten.\n* `Enter` in het document: Een link volgen.\n* `Shift+F10`: Het contextmenu openen.";

	#[test]
	fn every_line_after_an_added_one_keeps_its_own_translation() {
		let plan = plan(WAS, NOW, DUTCH).unwrap();
		assert_eq!(numbered_new_lines(&plan), "1: * `Ctrl+1` through `Ctrl+9`: Go to the first nine open documents.");
		let updated =
			assemble(NOW, &plan, "* `Ctrl+1` t/m `Ctrl+9`: Naar de eerste negen geopende documenten gaan.").unwrap();
		assert_eq!(
			updated,
			"### Aanvullende toetsen\n\n* `Delete` op het tabbladelement: Het geselecteerde tabblad sluiten.\n* `Ctrl+1` t/m `Ctrl+9`: Naar de eerste negen geopende documenten gaan.\n* `Enter` in het document: Een link volgen.\n* `Shift+F10`: Het contextmenu openen."
		);
	}

	#[test]
	fn an_edited_line_is_translated_again_and_a_removed_one_dropped() {
		let now = "### Extra keys\n\n* `Delete` on the tab control: Close the selected document tab.\n* `Shift+F10`: Open the context menu.";
		let plan = plan(WAS, now, DUTCH).unwrap();
		assert_eq!(new_line_count(&plan), 1);
		let updated =
			assemble(now, &plan, "* `Delete` op het tabbladelement: Het geselecteerde documenttabblad sluiten.")
				.unwrap();
		assert_eq!(
			updated,
			"### Aanvullende toetsen\n\n* `Delete` op het tabbladelement: Het geselecteerde documenttabblad sluiten.\n* `Shift+F10`: Het contextmenu openen."
		);
	}

	#[test]
	fn a_section_that_only_lost_lines_needs_no_request() {
		let now = "### Extra keys\n\n* `Shift+F10`: Open the context menu.";
		let plan = plan(WAS, now, DUTCH).unwrap();
		assert_eq!(new_line_count(&plan), 0);
		assert_eq!(
			assemble(now, &plan, "").unwrap(),
			"### Aanvullende toetsen\n\n* `Shift+F10`: Het contextmenu openen."
		);
	}

	/// A translator who split or merged lines leaves nothing to pair by.
	#[test]
	fn a_translation_with_a_different_line_count_cannot_be_planned() {
		assert!(plan(WAS, NOW, "### Aanvullende toetsen\n\n* `Delete`: Sluiten.").is_none());
	}

	#[test]
	fn a_new_line_gets_the_english_code_spans_back() {
		let plan = plan(WAS, NOW, DUTCH).unwrap();
		assert!(
			assemble(NOW, &plan, "* `Ctrl+1` t/m `Strg+9`: Naar de eerste negen documenten.")
				.unwrap()
				.contains("`Ctrl+1` t/m `Ctrl+9`")
		);
	}

	#[test]
	fn an_answer_with_the_wrong_number_of_lines_is_rejected() {
		let plan = plan(WAS, NOW, DUTCH).unwrap();
		assert!(assemble(NOW, &plan, "").is_err());
		assert!(assemble(NOW, &plan, "* a\n* b").is_err());
	}

	#[test]
	fn an_answer_that_breaks_the_outline_is_rejected() {
		let plan = plan(WAS, NOW, DUTCH).unwrap();
		assert!(assemble(NOW, &plan, "Naar de eerste negen documenten.").is_err());
	}
}
