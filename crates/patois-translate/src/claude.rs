//! Machine translation through the Claude API.
//!
//! A dedicated translation API takes only a string and a target language, which loses most of what
//! a catalog knows: the `TRANSLATORS:` notes that disambiguate a string, the `&` accelerators, and
//! the `\tCtrl+O` shortcuts. A model can be told about all three, and a check verifies each answer.
//! A result that fails the check is returned as `None` and left for the next run.

use std::{env, error::Error, fmt::Write as _, thread, time::Duration};

use patois_build::check::{check, check_plural};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
	App,
	markdown::{chunk_name, restore_code_spans, split_markdown, structure_mismatch},
	prompts::{
		markdown_system_prompt, phrase_system_prompt, plural_schema, plural_system_prompt, translations_schema,
		update_system_prompt,
	},
	update::{Line, assemble, new_line_count, numbered_new_lines, plan},
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

/// Short strings with the rules stated up front and a checked result suit the cheapest model. A
/// larger model buys better phrasing, not correctness.
const DEFAULT_MODEL: &str = "claude-haiku-4-5";

/// Keeps a failed request from losing more than one batch of a language.
const BATCH_LIMIT: usize = 60;

/// Smaller than [`BATCH_LIMIT`] because each entry comes back as several forms.
const PLURAL_BATCH_LIMIT: usize = 20;

const README_CHUNK_CHARS: usize = 6000;

const MAX_TOKENS: u32 = 16000;
const MARKDOWN_ATTEMPTS: usize = 3;

const MAX_ATTEMPTS: u32 = 4;

pub struct Phrase {
	pub source: String,
	pub context: Option<String>,
}

pub struct PluralPhrase {
	pub singular: String,
	pub plural: String,
	pub context: Option<String>,
}

pub struct Target<'a> {
	pub language: &'a str,
	pub style: Option<&'a str>,
}

#[derive(Default)]
pub struct Guidance<'a> {
	pub existing: Option<&'a str>,
	pub terms: &'a str,
}

pub struct ClaudeClient {
	api_key: String,
	model: String,
	app: App,
}

fn guidance_text(guidance: &Guidance) -> String {
	let mut text = String::new();
	if !guidance.terms.is_empty() {
		// Without these, a translated section can give a menu a name that's on no screen.
		let _ = write!(
			text,
			"\n\nThe app's interface already words these terms as follows. Where the document names one of them, such as a menu, a dialog, a button or an option, use exactly this wording. Where a term lists more than one, separated by \" / \", the interface uses each for a different control; use the one that fits what the document is describing:\n<interface_terms>\n{}\n</interface_terms>",
			guidance.terms
		);
	}
	if let Some(existing) = guidance.existing {
		// Without this, one new line rewords its whole section and undoes what reviewers settled
		// on.
		let _ = write!(
			text,
			"\n\nThis document is a newer version of text that is already translated, and this is the existing translation of the whole section it belongs to. Update that translation instead of translating afresh: keep every heading, sentence and list item whose English meaning did not change exactly as the existing translation words it, and translate only what is new or changed. Return the translation of the document alone, not of the rest of the section.\n<existing_translation>\n{existing}\n</existing_translation>"
		);
	}
	text
}

impl ClaudeClient {
	#[must_use]
	pub fn new(api_key: String, app: App) -> Self {
		let model = env::var("PATOIS_TRANSLATE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
		Self { api_key, model, app }
	}

	#[must_use]
	pub fn model(&self) -> &str {
		&self.model
	}

	// Low effort holds down thinking tokens on a task whose rules are all given up front.
	fn output_config(&self, format: &Value) -> Value {
		let mut config = json!({ "format": format });
		if supports_effort(&self.model) {
			config["effort"] = json!("low");
		}
		config
	}

	/// Returns one result for each phrase, in order, with `None` where a result failed [`check`].
	pub fn translate_phrases(
		&self,
		phrases: &[Phrase],
		target: &Target,
	) -> Result<Vec<Option<String>>, Box<dyn Error>> {
		let mut out = Vec::with_capacity(phrases.len());
		for chunk in phrases.chunks(BATCH_LIMIT) {
			out.extend(self.translate_chunk(chunk, target)?);
		}
		Ok(out)
	}

	// Separate from the call so that a test can check its shape, since the test suite makes no live
	// calls that would show a 400.
	fn phrase_request(&self, phrases: &[Phrase], target: &Target) -> Result<Value, Box<dyn Error>> {
		let items: Vec<Value> = phrases
			.iter()
			.enumerate()
			.map(|(i, p)| {
				p.context.as_ref().map_or_else(
					|| json!({ "id": i, "text": p.source }),
					|context| json!({ "id": i, "text": p.source, "context": context }),
				)
			})
			.collect();
		Ok(json!({
			"model": self.model,
			"max_tokens": MAX_TOKENS,
			"output_config": self.output_config(&json!({ "type": "json_schema", "schema": translations_schema() })),
			// The rules are the same for every batch of a language, so they can be cached.
			"system": [{
				"type": "text",
				"text": phrase_system_prompt(&self.app, target.style),
				"cache_control": { "type": "ephemeral" }
			}],
			"messages": [{
				"role": "user",
				"content": format!(
					"Target language: {}\n\nTranslate every entry:\n{}",
					target.language,
					serde_json::to_string_pretty(&items)?
				)
			}]
		}))
	}

	fn translate_chunk(&self, phrases: &[Phrase], target: &Target) -> Result<Vec<Option<String>>, Box<dyn Error>> {
		#[derive(Deserialize)]
		struct Item {
			id: usize,
			text: String,
		}
		#[derive(Deserialize)]
		struct Payload {
			translations: Vec<Item>,
		}
		let request = self.phrase_request(phrases, target)?;
		let text = self.send(&request)?;
		let payload: Payload = serde_json::from_str(&text)
			.map_err(|e| format!("Claude returned a response that did not match the schema: {e} (body: {text})"))?;
		// By id, not position: the schema doesn't make the array complete or ordered.
		let mut out: Vec<Option<String>> = vec![None; phrases.len()];
		for item in payload.translations {
			if let Some(slot) = out.get_mut(item.id) {
				*slot = check(&phrases[item.id].source, &item.text);
			}
		}
		Ok(out)
	}

	pub fn translate_plurals(
		&self,
		phrases: &[PluralPhrase],
		target: &Target,
		nplurals: usize,
		plural_rule: &str,
	) -> Result<Vec<Option<Vec<String>>>, Box<dyn Error>> {
		let mut out = Vec::with_capacity(phrases.len());
		for chunk in phrases.chunks(PLURAL_BATCH_LIMIT) {
			out.extend(self.translate_plural_chunk(chunk, target, nplurals, plural_rule)?);
		}
		Ok(out)
	}

	fn translate_plural_chunk(
		&self,
		phrases: &[PluralPhrase],
		target: &Target,
		nplurals: usize,
		plural_rule: &str,
	) -> Result<Vec<Option<Vec<String>>>, Box<dyn Error>> {
		#[derive(Deserialize)]
		struct Item {
			id: usize,
			forms: Vec<String>,
		}
		#[derive(Deserialize)]
		struct Payload {
			translations: Vec<Item>,
		}
		let language = target.language;
		let items: Vec<Value> = phrases
			.iter()
			.enumerate()
			.map(|(i, p)| {
				let mut item = json!({ "id": i, "singular": p.singular, "plural": p.plural });
				if let Some(context) = &p.context {
					item["context"] = json!(context);
				}
				item
			})
			.collect();
		let request = json!({
			"model": self.model,
			"max_tokens": MAX_TOKENS,
			"output_config": self.output_config(&json!({ "type": "json_schema", "schema": plural_schema() })),
			"system": [{
				"type": "text",
				"text": plural_system_prompt(&self.app, target.style),
				"cache_control": { "type": "ephemeral" }
			}],
			"messages": [{
				"role": "user",
				"content": format!(
					"Target language: {language}\n\
					 This language has {nplurals} plural forms. Its gettext rule is:\n{plural_rule}\n\n\
					 Return exactly {nplurals} forms for each entry, in index order: forms[i] is used \
					 for the counts where that rule yields i.\n\n{}",
					serde_json::to_string_pretty(&items)?
				)
			}]
		});
		let text = self.send(&request)?;
		let payload: Payload = serde_json::from_str(&text)
			.map_err(|e| format!("Claude returned a response that did not match the schema: {e} (body: {text})"))?;
		let mut out: Vec<Option<Vec<String>>> = vec![None; phrases.len()];
		for item in payload.translations {
			let Some(slot) = out.get_mut(item.id) else { continue };
			*slot = check_plural(&phrases[item.id].plural, &item.forms, nplurals);
		}
		Ok(out)
	}

	pub fn translate_markdown(
		&self,
		markdown: &str,
		guidance: &Guidance,
		target: &Target,
	) -> Result<String, Box<dyn Error>> {
		let mut translated: Vec<String> = Vec::new();
		for chunk in split_markdown(markdown, README_CHUNK_CHARS) {
			translated.push(self.translate_markdown_checked(&chunk, guidance, target)?);
		}
		let joined = translated.join("\n\n");
		Ok(restore_code_spans(markdown, &joined))
	}

	/// Translates one chunk and checks it against its source, and fails after
	/// [`MARKDOWN_ATTEMPTS`] tries. Half a readme is worse than none, because nobody who reads that
	/// language can tell which half is missing.
	fn translate_markdown_checked(
		&self,
		markdown: &str,
		guidance: &Guidance,
		target: &Target,
	) -> Result<String, Box<dyn Error>> {
		let mut last = String::new();
		let mut last_text = String::new();
		for attempt in 1..=MARKDOWN_ATTEMPTS {
			// A blind retry of the same request mostly returns the same answer, so each retry is
			// told what was wrong.
			let previous = (attempt > 1).then_some(last.as_str());
			let translated = self.translate_markdown_chunk(markdown, guidance, target, previous)?;
			let Some(why) = structure_mismatch(markdown, &translated) else {
				return Ok(translated);
			};
			eprintln!("{}: {why}; retrying ({attempt} of {MARKDOWN_ATTEMPTS})", chunk_name(markdown));
			last = why;
			last_text = translated;
		}
		// The counts don't say what went missing, so the log gets the last attempt.
		eprintln!("--- what came back, last attempt:\n{last_text}\n--- what was sent:\n{markdown}\n---");
		Err(format!(
			"{} came back incomplete {MARKDOWN_ATTEMPTS} times in {}: {last}",
			chunk_name(markdown),
			target.language
		)
		.into())
	}

	fn translate_markdown_chunk(
		&self,
		markdown: &str,
		guidance: &Guidance,
		target: &Target,
		previous_problem: Option<&str>,
	) -> Result<String, Box<dyn Error>> {
		let correction = previous_problem.map_or_else(String::new, |why| {
			format!(
				"\n\nYour previous attempt did not match the document: {why}. \
				 Return every heading and every list item this time, in the same order."
			)
		});
		// Text, not a `json_schema` response. A schema response is generated against a JSON
		// grammar, which cut German off at a closing quote mark on every attempt.
		let request = json!({
			"model": self.model,
			"max_tokens": MAX_TOKENS,
			"system": [{
				"type": "text",
				"text": markdown_system_prompt(&self.app, target.style),
				"cache_control": { "type": "ephemeral" }
			}],
			"messages": [{
				"role": "user",
				"content": format!(
					"Target language: {}{correction}{}\n\n<document>\n{markdown}\n</document>",
					target.language,
					guidance_text(guidance)
				)
			}]
		});
		Ok(unfenced(self.send(&request)?.trim()))
	}

	/// Updates the translation of a section whose English changed from `was` to `now`. See
	/// [`crate::update`].
	///
	/// Falls back to [`Self::translate_markdown`], because a section reworded in places is better
	/// than one that isn't updated.
	pub fn update_markdown(
		&self,
		was: &str,
		now: &str,
		existing: &str,
		terms: &str,
		target: &Target,
	) -> Result<String, Box<dyn Error>> {
		let fallback = || self.translate_markdown(now, &Guidance { existing: Some(existing), terms }, target);
		let Some(plan) = plan(was, now, existing) else {
			eprintln!(
				"{}: its translation does not line up with its English, translating it with the existing translation as guidance",
				chunk_name(now)
			);
			return fallback();
		};
		if new_line_count(&plan) == 0 {
			if let Ok(section) = assemble(now, &plan, "") {
				return Ok(section);
			}
			return fallback();
		}
		let mut previous: Option<String> = None;
		for attempt in 1..=MARKDOWN_ATTEMPTS {
			let request = self.update_request(now, existing, &plan, terms, target, previous.as_deref());
			let answer = unfenced(self.send(&request)?.trim());
			match assemble(now, &plan, &answer) {
				Ok(section) => return Ok(section),
				Err(why) => {
					eprintln!("{}: {why}; retrying the update ({attempt} of {MARKDOWN_ATTEMPTS})", chunk_name(now));
					previous = Some(why);
				}
			}
		}
		eprintln!("{}: no update fitted, translating it with the existing translation as guidance", chunk_name(now));
		fallback()
	}

	fn update_request(
		&self,
		now: &str,
		existing: &str,
		plan: &[Line],
		terms: &str,
		target: &Target,
		previous_problem: Option<&str>,
	) -> Value {
		let correction = previous_problem.map_or_else(String::new, |why| {
			format!("\n\nYour previous answer did not fit: {why}. Answer exactly one line per numbered line, keeping each line's Markdown.")
		});
		let guidance = guidance_text(&Guidance { existing: None, terms });
		json!({
			"model": self.model,
			"max_tokens": MAX_TOKENS,
			"system": [{
				"type": "text",
				"text": update_system_prompt(&self.app, target.style),
				"cache_control": { "type": "ephemeral" }
			}],
			"messages": [{
				"role": "user",
				"content": format!(
					"Target language: {}{correction}{guidance}\n\n<english_section>\n{now}\n</english_section>\n\n<existing_translation>\n{existing}\n</existing_translation>\n\n<lines_to_translate>\n{}\n</lines_to_translate>",
					target.language,
					numbered_new_lines(plan)
				)
			}]
		})
	}

	fn send(&self, request: &Value) -> Result<String, Box<dyn Error>> {
		let mut attempt = 0;
		loop {
			attempt += 1;
			let response = ureq::post(API_URL)
				.config()
				.http_status_as_error(false)
				.build()
				.header("x-api-key", &self.api_key)
				.header("anthropic-version", API_VERSION)
				.header("content-type", "application/json")
				.send_json(request);
			let mut response = match response {
				Ok(response) => response,
				Err(e) if attempt < MAX_ATTEMPTS => {
					backoff(attempt, None);
					let _ = e;
					continue;
				}
				Err(e) => return Err(format!("Claude request failed: {e}").into()),
			};
			let status = response.status();
			let retry_after =
				response.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
			let body = response.body_mut().read_to_string().unwrap_or_default();
			if status.is_success() {
				return first_text_block(&body);
			}
			let retryable = status.as_u16() == 429 || status.is_server_error();
			if retryable && attempt < MAX_ATTEMPTS {
				backoff(attempt, retry_after);
				continue;
			}
			return Err(format!("Claude request returned HTTP {status}: {body}").into());
		}
	}
}

fn backoff(attempt: u32, retry_after: Option<u64>) {
	let seconds = retry_after.unwrap_or_else(|| 2u64.pow(attempt));
	thread::sleep(Duration::from_secs(seconds.min(60)));
}

// The text isn't always the first block: with thinking on, a `thinking` block comes before it.
fn first_text_block(body: &str) -> Result<String, Box<dyn Error>> {
	let parsed: Value =
		serde_json::from_str(body).map_err(|e| format!("Claude response was not valid JSON: {e} (body: {body})"))?;
	// A refusal is a 200 with no usable content, which would otherwise read as a malformed
	// response.
	if parsed.get("stop_reason").and_then(Value::as_str) == Some("refusal") {
		return Err(
			format!("Claude declined the request: {}", parsed.get("stop_details").unwrap_or(&Value::Null)).into()
		);
	}
	parsed
		.get("content")
		.and_then(Value::as_array)
		.and_then(|blocks| {
			blocks
				.iter()
				.find(|b| b.get("type").and_then(Value::as_str) == Some("text"))
				.and_then(|b| b.get("text"))
				.and_then(Value::as_str)
		})
		.map(str::to_string)
		.ok_or_else(|| format!("Claude response had no text block (body: {body})").into())
}

// The Haiku tier returns a 400 for `effort`. This is "everything except Haiku" rather than a list,
// so that a newer model gets the parameter by default.
fn supports_effort(model: &str) -> bool {
	!model.contains("haiku")
}

/// Removes the code fence that a model sometimes wraps a whole answer in. The first line must open
/// a fence and the last must close one, so a section that begins with a fenced block keeps it.
fn unfenced(markdown: &str) -> String {
	let mut lines = markdown.lines();
	let (Some(first), Some(last)) = (lines.next(), markdown.lines().next_back()) else {
		return markdown.to_string();
	};
	if markdown.lines().count() < 2 || !first.starts_with("```") || last.trim() != "```" {
		return markdown.to_string();
	}
	let inner: Vec<&str> = markdown.lines().skip(1).collect();
	inner[..inner.len() - 1].join("\n")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_refusal_is_reported_as_a_refusal() {
		let body = r#"{"stop_reason":"refusal","stop_details":{"type":"refusal"},"content":[]}"#;
		let err = first_text_block(body).unwrap_err().to_string();
		assert!(err.contains("declined"), "got: {err}");
	}

	// With thinking on, a thinking block precedes the text block.
	#[test]
	fn the_text_block_is_found_past_a_thinking_block() {
		let body = r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"{}"}]}"#;
		assert_eq!(first_text_block(body).unwrap(), "{}");
	}

	// The request shape is otherwise only checked by the API itself, which the test suite
	// never talks to. This pins the parts that have to be exactly where they are.
	#[test]
	fn the_request_body_has_the_shape_the_api_expects() {
		let client = ClaudeClient { api_key: "test".to_string(), model: "test-model".to_string(), app: App::default() };
		let phrases = vec![
			Phrase { source: "&Settings".to_string(), context: Some("Menu item".to_string()) },
			Phrase { source: "Ready".to_string(), context: None },
		];
		let target = Target { language: "Russian", style: Some("Use the informal register.") };
		let request = client.phrase_request(&phrases, &target).unwrap();
		assert_eq!(request["model"], "test-model", "the configured model has to reach the request");
		assert_eq!(request["output_config"]["effort"], "low", "a model that accepts effort gets it");
		// `format` nests inside output_config; as a top-level `output_format` it is the
		// deprecated spelling and would be rejected.
		assert_eq!(request["output_config"]["format"]["type"], "json_schema");
		assert!(request["output_config"]["format"]["schema"].is_object());
		// The system prompt is a block list so it can carry cache_control, not a bare string.
		assert_eq!(request["system"][0]["cache_control"]["type"], "ephemeral");
		let system = request["system"][0]["text"].as_str().unwrap();
		assert!(system.ends_with("## Conventions for this language\n\nUse the informal register."), "got: {system}");
		assert_eq!(request["messages"][0]["role"], "user");
		let content = request["messages"][0]["content"].as_str().unwrap();
		assert!(content.contains("Target language: Russian"));
		assert!(content.contains("&Settings"), "the source string must reach the request");
		assert!(content.contains("Menu item"), "the translator note must reach the request");
		assert!(content.contains("\"id\": 0") && content.contains("\"id\": 1"), "every entry needs its id");
	}

	#[test]
	fn the_update_request_asks_only_for_the_new_lines() {
		let client = ClaudeClient { api_key: "test".to_string(), model: "test-model".to_string(), app: App::default() };
		let target = Target { language: "Dutch", style: Some("Address the reader as \"je\".") };
		let was = "## Help\n\n* `F1`: View help.";
		let now = "## Help\n\n* `F1`: View help.\n* `Ctrl+D`: Donate.";
		let existing = "## Hulp\n\n* `F1`: Help bekijken.";
		let plan = plan(was, now, existing).unwrap();
		let request = client.update_request(
			now,
			existing,
			&plan,
			"Help → Help",
			&target,
			Some("1 lines were asked for and 2 came back"),
		);
		assert!(request.get("output_config").is_none(), "the answer is plain lines, not JSON");
		let system = request["system"][0]["text"].as_str().unwrap();
		assert!(system.ends_with("Address the reader as \"je\"."), "got: {system}");
		let content = request["messages"][0]["content"].as_str().unwrap();
		assert!(
			content.contains("<lines_to_translate>\n1: * `Ctrl+D`: Donate.\n</lines_to_translate>"),
			"got: {content}"
		);
		assert!(
			content.contains("<existing_translation>\n## Hulp"),
			"the existing translation is there for its terms and tone"
		);
		assert!(content.contains("Help → Help"), "the interface terms reach the request");
		assert!(content.contains("did not fit: 1 lines"), "a retry is told what went wrong");
	}

	#[test]
	fn a_whole_answer_wrapped_in_a_fence_is_unwrapped() {
		let answer = "```markdown\n## Eins\n\nText.\n```";
		assert_eq!(unfenced(answer), "## Eins\n\nText.");
	}

	/// A section that really does open with a fenced block keeps it.
	#[test]
	fn a_fenced_block_inside_the_answer_is_left_alone() {
		let answer = "```\ncode\n```\n\nText after.";
		assert_eq!(unfenced(answer), answer);
	}

	#[test]
	fn an_answer_with_no_fence_is_unchanged() {
		assert_eq!(unfenced("## Eins\n\nText."), "## Eins\n\nText.");
	}

	// Sending `effort` to a model that does not take it is a 400 on every single request, so
	// this is correctness rather than tuning. Haiku is the default, which is what made it bite.
	#[test]
	fn effort_is_sent_only_to_models_that_accept_it() {
		assert!(!supports_effort("claude-haiku-4-5"));
		assert!(supports_effort("claude-opus-5"));
		assert!(supports_effort("claude-sonnet-5"));
	}

	#[test]
	fn the_opus_request_still_carries_effort() {
		let client =
			ClaudeClient { api_key: "test".to_string(), model: "claude-opus-5".to_string(), app: App::default() };
		let phrases = vec![Phrase { source: "Ready".to_string(), context: None }];
		let request = client.phrase_request(&phrases, &Target { language: "French", style: None }).unwrap();
		assert_eq!(request["output_config"]["effort"], "low");
		assert_eq!(request["output_config"]["format"]["type"], "json_schema");
	}

	// A typo here is a 404 on every request, and only at runtime.
	#[test]
	fn the_default_model_is_a_real_model_id() {
		assert_eq!(DEFAULT_MODEL, "claude-haiku-4-5");
	}

	#[test]
	fn a_phrase_without_a_note_carries_no_context_field() {
		let client = ClaudeClient { api_key: "test".to_string(), model: "test-model".to_string(), app: App::default() };
		let phrases = vec![Phrase { source: "Ready".to_string(), context: None }];
		let request = client.phrase_request(&phrases, &Target { language: "French", style: None }).unwrap();
		let content = request["messages"][0]["content"].as_str().unwrap();
		assert!(!content.contains("context"), "an absent note should be absent, not empty");
	}

	#[test]
	fn a_target_without_a_note_gets_only_the_base_prompt() {
		let client = ClaudeClient { api_key: "test".to_string(), model: "test-model".to_string(), app: App::default() };
		let phrases = vec![Phrase { source: "Ready".to_string(), context: None }];
		let request = client.phrase_request(&phrases, &Target { language: "French", style: None }).unwrap();
		let system = request["system"][0]["text"].as_str().unwrap();
		assert!(!system.contains("## Conventions for this language"), "got: {system}");
	}
}
