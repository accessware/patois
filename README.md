# patois

patois translates Rust desktop apps with [gettext](https://www.gnu.org/software/gettext/). You mark strings with `t()`, patois collects them into a template for translators, and the compiled translations are embedded in your binary.

Libraries can mark their strings too. A library calls `t()` and needs no setup of its own. The app that uses the library collects the library's strings into its own catalog, so one language setting covers both.

## Features

- Plain gettext files, which work with existing tools such as [Poedit](https://poedit.net/).
- Catalogs embedded in the binary as `msgfmt` wrote them, and read in place: nothing is decompressed or copied, and a language that isn't used is never read.
- Selection of the language that best fits the system. For example, a system set to `zh-Hans-CN` finds a catalog named `zh_CN`.
- Plural forms that follow each language's own rules.
- Optional [wxDragon](https://github.com/AllenDang/wxDragon) integration, which translates the stock strings of wxWidgets along with your own.
- Checks for translations that lose a placeholder, a menu accelerator, or a keyboard shortcut.
- Optional machine translation through the Claude API.
- String extraction from Swift and Kotlin, and asset generation for iOS and Android, for apps that share a catalog with a mobile version.

## Crates

| Crate | Use it for | Add it to |
| --- | --- | --- |
| [`patois`](https://docs.rs/patois) | Looking up translations at runtime. | `[dependencies]` |
| [`patois-build`](https://docs.rs/patois-build) | Extracting strings, compiling catalogs, and checking translations. | `[build-dependencies]`, or your `xtask` |
| [`patois-translate`](https://docs.rs/patois-translate) | Machine translation of catalogs and readmes. | Your `xtask` |
| [`patois-macros`](https://docs.rs/patois-macros) | Nothing directly. `patois` re-exports its macros. | None |

## Before you begin

An app needs Rust 1.95 or later, because `patois-build` does. A library that depends only on `patois` needs Rust 1.88 or later.

Install the [gettext tools](https://www.gnu.org/software/gettext/) and make sure that they're on your `PATH`. To install them, use one of the following commands:

- Windows: `scoop install gettext`
- macOS: `brew install gettext`
- Debian and Ubuntu: `sudo apt install gettext`

Without them, your app still builds, but with a Cargo warning and without translations.

## Translate an app

This section shows the complete process for a new app named `myapp`.

Add patois to `Cargo.toml`, and mark the package as containing translatable strings:

```toml
[package.metadata.patois]
translatable = true

[dependencies]
patois = "0.3"

[build-dependencies]
patois-build = "0.1"

[dev-dependencies]
patois-build = "0.1"
```

Mark your strings in `src/main.rs`, and embed the catalogs:

```rust
use patois::{nt, t};

patois::embed_domain!();

fn main() {
    patois::init_auto(env!("CARGO_PKG_NAME"));
    println!("{}", t("Hello"));
    let count = 3;
    println!("{}", nt("%d file", "%d files", count).replacen("%d", &count.to_string(), 1));
}
```

`t()` and `nt()` return a `String`. They return the English text until a translation exists.

To compile catalogs during the build, create `build.rs`:

```rust
fn main() {
    patois_build::compile_translations("po", "locale");
}
```

The compiled catalogs go to `locale/`. Add `/locale` to your `.gitignore` file.

To extract strings, create `examples/gen_pot.rs`. Running it writes the template, `po/myapp.pot`.

```rust
fn main() -> Result<(), Box<dyn std::error::Error>> {
    patois_build::gen_pot(".", "po", "myapp")
}
```

Create a catalog for each language, and translate it in a text editor or a tool such as Poedit. For example, for German:

```sh
msginit --input=po/myapp.pot --output-file=po/de.po --locale=de --no-translator
```

To try a language other than your system's, call `patois::set_locale("de")` after `init_auto`.

When your strings change, run `gen_pot` again, and then update each catalog:

```sh
msgmerge --update --backup=none --no-fuzzy-matching po/de.po po/myapp.pot
```

Keep `--no-fuzzy-matching`. Without it, `msgmerge` gives each new string the translation of a similar string and flags it fuzzy, and patois ships fuzzy entries. For details, see [How fuzzy entries work](#how-fuzzy-entries-work).

## Write translatable strings

Pass string literals. The extractor reads your source code and doesn't run it, so it can't see a string in a variable, and it reads only the first part of a concatenated string.

To insert a value, replace a placeholder in the result:

```rust
let title = t("Update to %s").replace("%s", version);
```

The translation checks understand the `%s`, `%d`, and `{}` placeholders.

To give translators context, put a comment directly above the call:

```rust
// TRANSLATORS: "Ready" is a status, not a button.
let status = t("Ready");
```

## Translate a library

A library doesn't embed or compile anything. Its strings become part of the catalog of each app that uses it.

Add patois to the library's `Cargo.toml`, and mark the package as translatable:

```toml
[package.metadata.patois]
translatable = true

[dependencies]
patois = "0.3"
```

Then call `t()` and `nt()` in the library.

When an app runs `patois_build::gen_pot`, it collects the strings of every dependency that has the `translatable` setting.

The library and the app must use compatible versions of `patois`. Otherwise, Cargo builds two copies with separate state, and the library's strings stay in English.

## Let users choose a language

`LanguageManager` lists the languages that are embedded in your app and switches between them:

```rust
let languages = patois::LanguageManager::new(env!("CARGO_PKG_NAME"));
for language in languages.available() {
    println!("{}: {}", language.code, language.name);
}
if !languages.set("de") {
    eprintln!("This app has no German catalog.");
}
```

`name` is the language's name in that language, such as `Deutsch`. After a language change, your app needs to show its strings again, because `t()` returns plain strings.

To load catalogs from a directory instead of embedding them, call `patois::init` in place of `init_auto`. Catalogs must be at `<locale_dir>/<locale>/LC_MESSAGES/<domain>.mo`. `LanguageManager` doesn't list catalogs loaded this way.

## Use patois with wxDragon

wxWidgets has its own translations for stock strings, such as the labels on standard buttons. The `wx` feature embeds those translations for the languages your app ships, and keeps them on the same language as your app.

Enable the feature in `Cargo.toml`:

```toml
[dependencies]
patois = { version = "0.3", features = ["wx"] }
wxdragon = "0.9"
```

Embed both sets of catalogs. Your build script must call `compile_translations`, as in [Translate an app](#translate-an-app), because the second macro relies on it to find the wxWidgets catalogs.

```rust
patois::embed_domain!();
patois::embed_wx_translations!();
```

Before you create any UI, initialize translations:

```rust
use patois::wx::WxTranslationManager;

let mut translations = WxTranslationManager::new(env!("CARGO_PKG_NAME"));
translations.initialize(WxStdCatalogLoader);
```

`embed_wx_translations!` generates the `WxStdCatalogLoader` type.

To fill a language selector, use `patois::wx::populate_language_choice` and `patois::wx::resolve_language_choice`.

## Translate catalogs with Claude

`patois-translate` fills the blank entries of your catalogs through the Claude API. It runs from your own tooling, not from your app.

Note: This feature sends your strings, your translator notes, and your readme to Anthropic. You need an API key, and each run uses your API credits.

Add `patois-translate` to your `xtask`, and call it after you regenerate the template:

```rust
use std::path::Path;

use patois_translate::{App, Options, Project};

let project = Project {
    root: Path::new("."),
    po_dir: Path::new("po"),
    pot: Path::new("po/myapp.pot"),
    readme: None,
    app: App {
        name: "MyApp".to_string(),
        description: "a desktop note-taking app".to_string(),
        proper_nouns: vec!["Markdown".to_string()],
    },
};
patois_translate::translate(&project, Options::default())?;
```

The API key comes from the `ANTHROPIC_API_KEY` environment variable. To see what a run would do without calling the API, set `Options::dry_run`.

Each answer is checked before it's written. A translation that loses a placeholder, an accelerator, or a shortcut is skipped, and the next run tries it again. The `msgmerge` tool must be on your `PATH`.

The following settings change what a run does:

- `PATOIS_TRANSLATE_MODEL`: an environment variable that selects a model other than the default, `claude-haiku-4-5`.
- `po/human-maintained-locales.txt`: locales that people translate, one on each line. The run skips them.
- `po/style/<lang>.md`: conventions for one language, such as how to address the reader. The run adds them to the instructions for that language.
- `Project::readme`: an English readme to translate into `<name>-<lang>.md` files. Only the lines that changed since the last run are sent again.

### How fuzzy entries work

patois gives the gettext `fuzzy` flag a different meaning from the usual one. A fuzzy entry is a machine translation that no person has reviewed, and it does ship: `compile_translations` passes `--use-fuzzy` to `msgfmt`. To mark a translation as reviewed, remove its `fuzzy` flag.

If you use translation tools that treat fuzzy entries as unfinished, they show machine translations as needing review, which is accurate.

## Check translations

`patois-build` finds mistakes that gettext doesn't report. Run these checks in your tests, so that CI catches new mistakes:

- `patois_build::lint::check_sources` finds `t("a " + "b")`. The extractor keeps only the first literal, so the string can't be translated.
- `patois_build::lint::check_catalogs` finds translations that end in a keyboard shortcut that their source doesn't have.
- `patois_build::check::is_damaged` finds a translation that lost or added a placeholder, an accelerator, or a shortcut.

To credit your translators, `patois_build::credits` reads their names from the `Last-Translator` header of each catalog, without their email addresses.

## Share a catalog with a mobile app

If your app has iOS or Android versions, `patois-build` can keep their strings in the same catalog. Your Swift or Kotlin code needs its own `t()` and `nt()` functions that read the generated assets.

- `patois_build::extend_pot_from_source_dirs` adds strings from Swift or Kotlin source to the template. Call it after `gen_pot`.
- `patois_build::prune_pot_from_source_dirs` removes the strings that those sources no longer use.
- `patois_build::gen_ios_strings` generates `Localizable.strings` files.
- `patois_build::gen_android_strings` generates JSON assets.

## Limitations

- The active language applies to the whole process. This suits a desktop app, but not a server that handles users with different languages.
- The source language must be English.
- `msgctxt` isn't supported, so one English string has one translation in each language. To translate "Open" differently as a verb and as an adjective, use different English strings.
- patois has no formatting function. If a string has two identical placeholders, translators can't reorder them. Use different placeholders, such as `%s` and `%d`.
- The translation checks don't understand named placeholders such as `{name}`.

## Upgrade from 0.2

- Upgrade the app and every library it uses that depends on `patois` together. Until a library upgrades, its strings stay in English in an app on 0.3.
- The `t!` and `nt!` macros are gone. Use `t()` and `nt()`. The extractor never collected strings from the macros, so those strings could not be translated.
- `t()` and `nt()` use only the active domain. A library's embedded catalogs aren't searched unless the library's domain is the active one, so collect library strings into the app's catalog instead.
- `get_locale` is now `locale`.
- The `ui` feature and module are now `wx`.
- `patois::ui::best_available_language` is now `patois::best_available_language`.
- `WxTranslationManager::initialize` no longer returns a value.
- Catalog directories now match regardless of case and separator, so a catalog named `pt_br` loads. It didn't load before.
- `compile_translations` now passes the build profile to `embed_wx_translations!`, so you can remove any `cargo:rustc-env=PROFILE=...` line from your build script.

## Develop patois

The `wx` feature builds wxWidgets, so the first build with `--all-features` takes a while. Formatting needs the nightly toolchain.

## License

patois is licensed under the [MIT license](LICENSE).
