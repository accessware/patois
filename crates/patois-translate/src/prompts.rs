use std::iter;

use serde_json::{Value, json};

use crate::App;

pub fn translations_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"translations": {
				"type": "array",
				"items": {
					"type": "object",
					"properties": {
						"id": { "type": "integer" },
						"text": { "type": "string" }
					},
					"required": ["id", "text"],
					"additionalProperties": false
				}
			}
		},
		"required": ["translations"],
		"additionalProperties": false
	})
}

// The form count isn't in the schema, because the API accepts a `minItems` of only 0 or 1. The
// prompt states the count, and `check_plural` enforces it.
pub fn plural_schema() -> Value {
	json!({
		"type": "object",
		"properties": {
			"translations": {
				"type": "array",
				"items": {
					"type": "object",
					"properties": {
						"id": { "type": "integer" },
						"forms": {
							"type": "array",
							"items": { "type": "string" },
							"minItems": 1
						}
					},
					"required": ["id", "forms"],
					"additionalProperties": false
				}
			}
		},
		"required": ["translations"],
		"additionalProperties": false
	})
}

pub fn phrase_system_prompt(app: &App, style: Option<&str>) -> String {
	with_style(&fill(include_str!("../prompts/phrase.md"), app), style)
}

pub fn plural_system_prompt(app: &App, style: Option<&str>) -> String {
	with_style(&fill(include_str!("../prompts/plural.md"), app), style)
}

pub fn markdown_system_prompt(app: &App, style: Option<&str>) -> String {
	with_style(&fill(include_str!("../prompts/markdown.md"), app), style)
}

pub fn update_system_prompt(app: &App, style: Option<&str>) -> String {
	with_style(&fill(include_str!("../prompts/update.md"), app), style)
}

#[allow(clippy::literal_string_with_formatting_args, reason = "the templates are Markdown files, not format strings")]
fn fill(template: &str, app: &App) -> String {
	let proper_nouns: Vec<&str> =
		iter::once(app.name.as_str()).chain(app.proper_nouns.iter().map(String::as_str)).collect();
	template
		.replace("{name}", &app.name)
		.replace("{description}", &app.description)
		.replace("{proper_nouns}", &proper_nouns.join(", "))
}

fn with_style(base: &str, style: Option<&str>) -> String {
	let base = base.trim_end();
	style
		.map(str::trim)
		.filter(|note| !note.is_empty())
		.map_or_else(|| base.to_string(), |note| format!("{base}\n\n## Conventions for this language\n\n{note}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_plural_schema_asks_only_for_a_non_empty_array() {
		let schema = plural_schema();
		let forms = &schema["properties"]["translations"]["items"]["properties"]["forms"];
		assert_eq!(forms["minItems"], 1);
		assert!(forms["maxItems"].is_null());
	}

	fn app() -> App {
		App {
			name: "Paperback".to_string(),
			description: "a desktop ebook and document reader".to_string(),
			proper_nouns: vec!["EPUB".to_string(), "PDF".to_string()],
		}
	}

	#[test]
	fn without_a_note_each_prompt_is_its_base_text() {
		assert!(phrase_system_prompt(&app(), None).contains("keyboard accelerator"));
		assert!(plural_system_prompt(&app(), None).contains("exactly the number of forms"));
		assert!(markdown_system_prompt(&app(), None).contains("Preserve the Markdown structure"));
		assert!(!phrase_system_prompt(&app(), None).contains("## Conventions for this language"));
	}

	#[test]
	fn the_app_is_filled_in() {
		for prompt in [
			phrase_system_prompt(&app(), None),
			plural_system_prompt(&app(), None),
			markdown_system_prompt(&app(), None),
		] {
			assert!(prompt.contains("Paperback, a desktop ebook and document reader"), "got: {prompt}");
			assert!(prompt.contains("Leave proper nouns alone: Paperback, EPUB, PDF,"), "got: {prompt}");
			assert!(!prompt.contains("{name}") && !prompt.contains("{proper_nouns}"), "got: {prompt}");
		}
	}

	#[test]
	fn a_style_note_is_appended_under_its_own_heading() {
		let note = "Address the reader as \"je\".\n";
		for prompt in [
			phrase_system_prompt(&app(), Some(note)),
			plural_system_prompt(&app(), Some(note)),
			markdown_system_prompt(&app(), Some(note)),
		] {
			let (base, tail) = prompt.split_once("\n\n## Conventions for this language\n\n").expect("heading");
			assert!(base.ends_with("take precedence over the general rules."), "got: {base}");
			assert_eq!(tail, "Address the reader as \"je\".");
		}
	}
}
