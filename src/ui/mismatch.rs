//! The screen shown when the interface and the daemon do not speak the same protocol.
//!
//! A dead end on purpose. The two halves of the player talk over a socket, and a daemon left
//! running from an older build can misread what this one sends — so rather than carrying on and
//! playing the wrong thing, the interface says which versions it found and stops. Restarting the
//! daemon has already been tried by the time this shows; what is left is for the user to install
//! the two together.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};

use crate::theme::Theme;

/// What the screen asks the application to do next.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MismatchOutcome {
    /// Close the player, and stop the daemon on the way out.
    Quit,
}

/// The versions that did not match, and how to say so.
#[derive(Debug)]
pub struct MismatchPane {
    /// What this build speaks.
    ours: u32,
    /// What the daemon speaks, or `None` for one too old to be asked.
    theirs: Option<u32>,
    /// Colours to draw with.
    theme: Theme,
}

impl MismatchPane {
    /// The screen for an interface speaking `ours` that found a daemon speaking `theirs`.
    pub fn new(ours: u32, theirs: Option<u32>) -> Self {
        MismatchPane { ours, theirs, theme: Theme::default() }
    }

    /// Draw with `theme` rather than whatever was set before.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// What this build speaks.
    pub fn ours(&self) -> u32 {
        self.ours
    }

    /// What the daemon speaks, if it could say.
    pub fn theirs(&self) -> Option<u32> {
        self.theirs
    }

    /// Any key press closes the player. There is nothing else this screen can do.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<MismatchOutcome> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        Some(MismatchOutcome::Quit)
    }

    /// How the daemon's version reads, for a daemon that could not name one.
    fn theirs_text(&self) -> String {
        match self.theirs {
            Some(version) => format!("IPC version {version}"),
            // A daemon that does not know the question predates the answer, which places it before
            // version 1 without saying where. The rest of that is said on the line below, since
            // this one has a label in front of it and the box is only so wide.
            None => "an older version".to_string(),
        }
    }

    /// Draw the screen centered in `area`.
    pub fn render(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.theme.error())
            .title(Line::from(" version mismatch ").centered().style(self.theme.error()))
            .title_bottom(
                Line::from(" any key closes ogma and stops the daemon ")
                    .centered()
                    .style(self.theme.muted()),
            )
            .padding(Padding::uniform(1));

        let mut lines = vec![
            Line::from("ogma and ogma-daemon are different builds.").style(self.theme.text()),
            Line::from(""),
            Line::from(vec![
                Span::from("ogma speaks        ").style(self.theme.muted()),
                Span::from(format!("IPC version {}", self.ours))
                    .style(self.theme.accent().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(vec![
                Span::from("ogma-daemon speaks ").style(self.theme.muted()),
                Span::from(self.theirs_text())
                    .style(self.theme.error().add_modifier(Modifier::BOLD)),
            ]),
        ];

        if self.theirs.is_none() {
            lines.push(
                Line::from("                   too old to say which").style(self.theme.muted()),
            );
        }

        lines.extend([
            Line::from(""),
            Line::from("Restarting the daemon did not change it,").style(self.theme.text()),
            Line::from("so the ogma-daemon being started is not").style(self.theme.text()),
            Line::from("from this build.").style(self.theme.text()),
            Line::from(""),
            Line::from("Install ogma, ogma-daemon and ogma-cmd").style(self.theme.text()),
            Line::from("together, into one directory.").style(self.theme.text()),
        ]);

        // Tall enough for the text, the padding and the borders; wide enough for the bottom title,
        // which is the longest line in the box.
        let height = lines.len() as u16 + 4;
        let pane_area = area.centered(Constraint::Length(50), Constraint::Length(height));

        let inner = block.inner(pane_area);
        frame.render_widget(block, pane_area);
        frame.render_widget(Paragraph::new(lines), inner);
    }
}
