//! The editor screen, used for the environment, Wi-Fi passwords, the account password (shown as dots),
//! names and the Library's edit mode's fields. It has a line saying what is being edited, the field with
//! the text so far (only its end if it is long), Cancel and the save button, and the keyboard below. The
//! keyboard's keys type into the field; Enter saves, Esc cancels and Ctrl+V pastes the clipboard's text.
//! Each purpose has its [`face`]; the tab the editor opens in draws it ([`draw`]). Typing a key chord for
//! a keymap uses the same layout ([`field`]).

use std::fmt::{self, Display, Formatter, Write};

use clamshell_state::bottom::editor::{KeymapName, Purpose};
use clamshell_state::library_view::edit::PlaceName;
use clamshell_state::library_view::files::FileName;
use clamshell_state::profiles::picker::ProfileName;
use clamshell_state::setup::account::PasswordStep;
use clamshell_state::{EditorId, Id, State};

use super::keyboard;
use clamshell_ui::gfx::{Align, Keep, LINE, Rect};
use clamshell_ui::toolkit::Frame;
use clamshell_ui::toolkit::format::Fmt;
use clamshell_ui::toolkit::style::{self, RADIUS, SURFACE_HI, Text, Tone};

/// The field's row height as a fraction of the area's height, the field's share of the row's width, and
/// the text sizes of the field and buttons as fractions of the row's height.
const ROW_H: f32 = 0.1;
const FIELD_W: f32 = 0.7;
const FIELD_TEXT: f32 = 0.42;
const BUTTON_TEXT: f32 = 0.36;

/// What the editor shows for what it edits: the line saying what it is, the save button's label, and
/// whether the text is secret, shown as dots.
pub(super) struct Face {
    pub(super) title: Fmt<192>,
    pub(super) save: &'static str,
    pub(super) secret: bool,
}

/// The face of an editor for `purpose`. Every purpose has one.
pub(super) fn face(s: &State, purpose: &Purpose) -> Face {
    let face = |title: fmt::Arguments, save| Face { title: Fmt::of(title), save, secret: false };
    match purpose {
        Purpose::Env { entry } => {
            let title = match entry {
                Some(id) => s.entry(id).map_or("this game", |g| g.title.as_str()),
                None => "all games",
            };
            face(format_args!("Environment for {title}: KEY=value, space-separated"), "Save")
        }
        Purpose::Password { ssid } => face(format_args!("Password for {ssid}"), "Join"),
        Purpose::KeymapName(KeymapName::NewLayer(..)) => face(format_args!("Name of the new layer"), "Save"),
        Purpose::KeymapName(KeymapName::RenameLayer(id, layer)) => {
            let name = s.keymapping().keymaps().get(*id).and_then(|k| k.layer_name(*layer)).unwrap_or("the layer");
            face(format_args!("New name for {name}"), "Save")
        }
        Purpose::ProfileName(kind, name) => match name {
            ProfileName::New => face(format_args!("Name of the new {}", kind.noun()), "Save"),
            ProfileName::Rename(id) => face(format_args!("New name for {}", s.profile_name(*kind, *id)), "Save"),
            ProfileName::Duplicate(id) => face(format_args!("Name of the copy of {}", s.profile_name(*kind, *id)), "Save"),
            ProfileName::SaveAs(_) => face(format_args!("A name for the copy (other games can use it)"), "Save"),
        },
        Purpose::FileName(FileName::Rename(_)) | Purpose::PlaceName(PlaceName::Rename(_)) => face(format_args!("New name"), "Rename"),
        Purpose::FileName(FileName::NewFolder(_)) | Purpose::PlaceName(PlaceName::New(Some(_))) => face(format_args!("New folder's name"), "Make"),
        Purpose::FileName(FileName::Extract(_)) => face(format_args!("Extract to"), "Extract"),
        Purpose::PlaceName(PlaceName::New(None)) => face(format_args!("New tab's name"), "Make"),
        Purpose::AccountPassword(step) => {
            let (title, save) = match step {
                PasswordStep::Current => ("Current password", "Next"),
                PasswordStep::New { current: Some(_) } => ("New password (8 characters at least)", "Next"),
                PasswordStep::New { current: None } => ("A password, for sudo and remote login (8 characters at least)", "Next"),
                PasswordStep::Again { .. } => ("The same password again", "Set"),
            };
            Face { secret: true, ..face(format_args!("{title}"), save) }
        }
        Purpose::FormField(field, _) => face(format_args!("{}", field.label()), "Set"),
        Purpose::SaveName => face(format_args!("Save as"), "Set"),
    }
}

/// The editor open in the tab shown, as its purpose's face says, with the keyboard below. Nothing is drawn
/// without one.
pub(super) fn draw(f: &mut Frame) {
    let s = f.s;
    let Some(editor) = s.bottom().editor() else {
        return;
    };
    let face = face(s, &editor.purpose);
    let dots;
    let text = if face.secret {
        dots = Fmt::<256>::of(format_args!("{}", Dots(editor.text.chars().count())));
        dots.as_str()
    } else {
        editor.text.as_str()
    };
    field_with(f, &face.title, text, (Id::Editor(EditorId::Cancel), "Cancel"), (Id::Editor(EditorId::Save), face.save), editor.snippets());
}

/// A secret as dots, one per character.
struct Dots(usize);

impl Display for Dots {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        for _ in 0..self.0 {
            f.write_char('•')?;
        }
        Ok(())
    }
}

/// Draws the editor's layout: `title`, a field with `text` and a caret, and the two buttons. `cancel` and
/// `save` give each button's target and label. The keyboard goes below.
pub(super) fn field(f: &mut Frame, title: &str, text: &str, cancel: (Id, &str), save: (Id, &str)) {
    field_with(f, title, text, cancel, save, &[]);
}

/// [`field`], with a row of `snippets` between the field and the keyboard, each a key that types it in.
fn field_with(f: &mut Frame, title: &str, text: &str, cancel: (Id, &str), save: (Id, &str), snippets: &[&str]) {
    let body = f.body();
    let pad = f.pad();
    let area = style::content(body).shifted(0.0, pad * 0.5);
    let size = f.text(Text::Small);
    f.c.text_fit(area.top_line(size), size, Tone::Dim, title, Keep::Start);

    // The field, with a caret after the text, showing only the end when the text is too long. Cancel and
    // save go beside it.
    let mut row = Rect::new(area.x, area.y + size * 1.6, area.w, f.body().h * ROW_H);
    let field = row.take_left(row.w * FIELD_W);
    f.c.rounded(field, RADIUS, SURFACE_HI);
    let text_size = field.h * FIELD_TEXT;
    let text_area = Rect::new(field.x + pad * 0.6, field.y, field.w - pad * 1.5, field.h);
    let tw = f.c.text_fit(text_area, text_size, Tone::High, text, Keep::End);
    let ty = field.y + (field.h - text_size * LINE) / 2.0;
    f.c.fill(Rect::new(text_area.x + tw + 2.0, ty, 2.0, text_size * LINE), Tone::Accent);

    let buttons = Rect::new(row.x + pad * 0.5, row.y, row.w - pad * 0.5, row.h);
    let (cancel_id, cancel_label) = cancel;
    f.back_over(cancel_id.clone());
    let cancel = buttons.cell(2, 1, pad * 0.5, 0);
    f.text_button(cancel_id, cancel, cancel.h * BUTTON_TEXT, Tone::Fn, cancel_label, Align::Center);
    let (save_id, save_label) = save;
    let ok = buttons.cell(2, 1, pad * 0.5, 1);
    f.text_button(save_id, ok, ok.h * BUTTON_TEXT, Tone::Fn, save_label, Align::Center);

    let mut keys_y = field.y + field.h + pad;
    if !snippets.is_empty() {
        let line = Rect::new(body.x + pad * 0.5, keys_y, body.w - pad, field.h * 0.8);
        for (i, snippet) in snippets.iter().enumerate() {
            let cell = line.cell(snippets.len().max(4), 1, pad * 0.5, i);
            // A space shows as a word, being a key of its own here.
            let label = if *snippet == " " { "space" } else { snippet };
            f.text_button(Id::Editor(EditorId::Insert(i as u8)), cell, cell.h * BUTTON_TEXT, Tone::Fn, label, Align::Center);
        }
        keys_y += line.h + pad * 0.5;
    }
    keyboard::draw(f, Rect::new(body.x, keys_y, body.w, body.y + f.body().h - keys_y));
}

#[cfg(test)]
mod tests {
    use super::*;
    use clamshell_state::Init;
    use clamshell_state::kit::secret::Secret;
    use clamshell_state::library_view::form::Field;
    use clamshell_state::profiles::picker::Kind;

    /// Every purpose an editor may be opened for is titled and has its save button's label, and only a
    /// password is secret.
    #[test]
    fn every_purpose_has_a_face() {
        let s = State::new(Init { library: Some(clamshell_library::demo::library()), ..Init::default() });
        let secret = || Secret("x".into());
        let purposes = [
            Purpose::Env { entry: None },
            Purpose::Env { entry: Some("zelda-lttp".into()) },
            Purpose::Password { ssid: "home".into() },
            Purpose::KeymapName(KeymapName::NewLayer(0, None)),
            Purpose::KeymapName(KeymapName::RenameLayer(0, clamshell_state::keymap::Layer(1))),
            Purpose::ProfileName(Kind::Keymap, ProfileName::New),
            Purpose::ProfileName(Kind::Curve, ProfileName::Rename(0)),
            Purpose::ProfileName(Kind::Keymap, ProfileName::Duplicate(0)),
            Purpose::ProfileName(Kind::Curve, ProfileName::SaveAs(0)),
            Purpose::FileName(FileName::Rename("a".into())),
            Purpose::FileName(FileName::NewFolder("a".into())),
            Purpose::FileName(FileName::Extract("a".into())),
            Purpose::AccountPassword(PasswordStep::Current),
            Purpose::AccountPassword(PasswordStep::New { current: None }),
            Purpose::AccountPassword(PasswordStep::New { current: Some(secret()) }),
            Purpose::AccountPassword(PasswordStep::Again { current: None, new: secret() }),
            Purpose::PlaceName(PlaceName::Rename("a".into())),
            Purpose::PlaceName(PlaceName::New(None)),
            Purpose::PlaceName(PlaceName::New(Some("a".into()))),
            Purpose::FormField(Field::Launch, 0),
            Purpose::SaveName,
        ];
        for purpose in purposes {
            let face = face(&s, &purpose);
            assert!(!face.title.is_empty() && !face.save.is_empty(), "{purpose:?}");
            assert_eq!(face.secret, matches!(purpose, Purpose::AccountPassword(_)), "{purpose:?}");
        }
    }
}
