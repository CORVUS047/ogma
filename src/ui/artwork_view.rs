//! Drawing cover art in a terminal.
//!
//! Each cell holds an upper half block: the foreground colour paints the top pixel, the background
//! the bottom one. That doubles the vertical resolution and makes the pixels roughly square, since a
//! character cell is about twice as tall as it is wide.

use std::path::PathBuf;

use image::imageops::FilterType;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Widget;

use crate::meta::Artwork;
use crate::theme::Theme;

/// The half block that splits a cell into two pixels.
const UPPER_HALF: &str = "▀";

/// A decoded image, sized for the area it was drawn into.
struct Decoded {
    /// What was decoded, and at what size, so the work is repeated only when one of them changes.
    key: (PathBuf, u16, u16),
    /// Row-major cells, each the colour of its top and bottom pixel.
    cells: Vec<Vec<(Color, Color)>>,
}

/// Holds the decoded cover art between frames.
///
/// Decoding a JPEG on every keystroke would be wasteful, so the result is kept until the song or the
/// available area changes.
#[derive(Default)]
pub struct ArtworkView {
    decoded: Option<Decoded>,
}

impl std::fmt::Debug for ArtworkView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArtworkView")
            .field("decoded", &self.decoded.is_some())
            .finish()
    }
}

impl ArtworkView {
    pub fn new() -> Self {
        Self::default()
    }

    /// Draw `artwork` centered in `area`, or a placeholder when there is none to draw.
    ///
    /// `key` identifies the song, so the cache knows when the picture has changed.
    pub fn render(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        key: &std::path::Path,
        artwork: Option<&Artwork>,
        theme: &Theme,
    ) {
        // Square pixels means twice as many columns as rows.
        let rows = area.height.min(area.width / 2);
        if rows == 0 {
            return;
        }

        let columns = rows * 2;
        let target = Rect {
            x: area.x + (area.width - columns) / 2,
            y: area.y + (area.height - rows) / 2,
            width: columns,
            height: rows,
        };

        let Some(artwork) = artwork else {
            self.decoded = None;
            placeholder(frame, target, theme);
            return;
        };

        let cache_key = (key.to_path_buf(), columns, rows);

        if self.decoded.as_ref().map(|decoded| &decoded.key) != Some(&cache_key) {
            match decode(&artwork.data, columns, rows) {
                Some(cells) => self.decoded = Some(Decoded { key: cache_key, cells }),
                None => {
                    // Undecodable art is no reason to lose the screen.
                    self.decoded = None;
                    placeholder(frame, target, theme);
                    return;
                }
            }
        }

        let Some(decoded) = &self.decoded else {
            return;
        };

        let buffer = frame.buffer_mut();

        for (row, cells) in decoded.cells.iter().enumerate() {
            for (column, (top, bottom)) in cells.iter().enumerate() {
                let Some(cell) = buffer.cell_mut((target.x + column as u16, target.y + row as u16))
                else {
                    continue;
                };

                cell.set_symbol(UPPER_HALF);
                cell.set_style(Style::new().fg(*top).bg(*bottom));
            }
        }
    }
}

/// Decode and shrink the image to one colour pair per cell.
fn decode(bytes: &[u8], columns: u16, rows: u16) -> Option<Vec<Vec<(Color, Color)>>> {
    let image = image::load_from_memory(bytes).ok()?;

    // Two pixel rows per cell row.
    let scaled = image
        .resize_exact(u32::from(columns), u32::from(rows) * 2, FilterType::Triangle)
        .to_rgb8();

    let cells = (0..rows)
        .map(|row| {
            (0..columns)
                .map(|column| {
                    let top = scaled.get_pixel(u32::from(column), u32::from(row) * 2);
                    let bottom = scaled.get_pixel(u32::from(column), u32::from(row) * 2 + 1);

                    (
                        Color::Rgb(top[0], top[1], top[2]),
                        Color::Rgb(bottom[0], bottom[1], bottom[2]),
                    )
                })
                .collect()
        })
        .collect();

    Some(cells)
}

/// A framed note where the art would be, so the layout does not jump about.
fn placeholder(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    use ratatui::widgets::{Block, BorderType};

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(theme.border(false));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    if inner.height == 0 {
        return;
    }

    // Centered vertically in whatever room the border left.
    let middle = Rect { y: inner.y + inner.height / 2, height: 1, ..inner };
    frame.render_widget(Line::from("♪ no artwork").style(theme.muted()).centered(), middle);
}
