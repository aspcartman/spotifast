//! Right-to-left text support for egui.
//!
//! The crmne egui fork (emilk/egui#8577) used to split font
//! runs where the Unicode bidi level changes and shapes each in its resolved
//! direction, so letters join and brackets mirror, but it still places the
//! shaped runs left to right in logical order. [`reorder`] moves them into
//! visual order after layout. The glyph vector stays in logical order with
//! each glyph's `rtl` flag set, so carets and hit-testing follow the text.
//!
//! [`layout`] handles wrapped and truncated text one row at a time and
//! places ellipses at the left reading edge.

use std::sync::Arc;

use egui::text::LayoutJob;
use egui::{Align, Align2, Color32, FontId, Galley, Painter, Pos2, Rect, pos2};
use unicode_bidi::{BidiClass, bidi_class};

/// The character that marks a cut.
pub const ELLIPSIS: char = '\u{2026}';

/// Whether the text reads right to left: decided by its first strong
/// character, the way the bidi algorithm decides a paragraph's direction.
/// ASCII never does, and most text is ASCII, so that check comes first and
/// costs nothing per frame.
pub fn is_rtl(text: &str) -> bool {
    if text.is_ascii() {
        return false;
    }
    for character in text.chars() {
        match bidi_class(character) {
            BidiClass::L => return false,
            BidiClass::R | BidiClass::AL => return true,
            _ => {}
        }
    }
    false
}

/// Whether the text holds a right-to-left letter anywhere, so its runs need
/// reordering even when the paragraph reads left to right.
pub fn has_rtl(text: &str) -> bool {
    !text.is_ascii() && text.chars().any(is_strong_rtl)
}

/// The edge the text should hug.
pub fn halign_for(text: &str) -> Align {
    if is_rtl(text) {
        Align::RIGHT
    } else {
        Align::LEFT
    }
}

/// One line of `text`, unwrapped, in visual order.
pub fn layout_line(
    painter: &Painter,
    text: impl Into<String>,
    font: FontId,
    color: Color32,
) -> Arc<Galley> {
    let mut galley = painter.layout_no_wrap(text.into(), font, color);
    reorder(&mut galley);
    galley
}

/// Lays `text` out within `wrap_width`, on at most `max_rows` rows, with
/// `overflow` marking a cut, in reading order for either direction. The
/// galley's `halign` says which edge to anchor it at; [`galley_pos`] does
/// that for a painter, and `egui::Label` does it on its own.
pub fn layout(
    painter: &Painter,
    text: &str,
    font: FontId,
    color: Color32,
    wrap_width: f32,
    max_rows: usize,
    overflow: Option<char>,
) -> Arc<Galley> {
    if !has_rtl(text) {
        let mut job = LayoutJob::simple(text.to_owned(), font, color, wrap_width);
        job.wrap.max_rows = max_rows;
        job.wrap.break_anywhere = false;
        job.wrap.overflow_character = overflow;
        return painter.layout_job(job);
    }
    // The rows are found here, whole words at a time, and each becomes a
    // paragraph of its own, so a paragraph's first row still holds its first
    // words and the cut lands where it is read last. Measuring words is the
    // dependable way to know what fits: epaint shapes them itself.
    let width = |piece: &str| {
        painter
            .layout_no_wrap(piece.to_owned(), font.clone(), color)
            .size()
            .x
    };
    let rows = break_rows(text, wrap_width, max_rows, overflow, width);
    let mut job = LayoutJob::simple(rows.join("\n"), font, color, f32::INFINITY);
    if text.split('\n').any(is_rtl) {
        job.halign = Align::RIGHT;
    }
    let mut galley = painter.layout_job(job);
    reorder(&mut galley);
    galley
}

/// Fills rows with whole words up to `wrap_width`, at most `max_rows` of
/// them, and ends a cut with `overflow`. The rows come back in logical
/// order, one string each.
fn break_rows(
    text: &str,
    wrap_width: f32,
    max_rows: usize,
    overflow: Option<char>,
    width: impl Fn(&str) -> f32,
) -> Vec<String> {
    let max_rows = max_rows.max(1);
    let space = width(" ");
    let mut rows: Vec<(String, f32)> = Vec::new();
    let mut row = (String::new(), 0.0_f32);
    let mut cut = false;
    'paragraphs: for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            if rows.len() + 1 >= max_rows {
                cut = true;
                break;
            }
            rows.push(std::mem::take(&mut row));
        }
        for word in paragraph.split_whitespace() {
            let word_width = width(word);
            if !row.0.is_empty() && row.1 + space + word_width > wrap_width {
                if rows.len() + 1 >= max_rows {
                    cut = true;
                    break 'paragraphs;
                }
                rows.push(std::mem::take(&mut row));
            }
            if !row.0.is_empty() {
                row.0.push(' ');
                row.1 += space;
            }
            row.0.push_str(word);
            row.1 += word_width;
        }
    }
    rows.push(row);
    // The last row can be wider than the column even when nothing was cut:
    // the first word of a row is taken whatever it measures, because a row
    // has to hold something, and a word longer than the column has nowhere
    // to break. `layout` hands the finished rows to epaint with no wrap
    // width of its own -- the fitting is meant to have happened here -- so a
    // row left too wide is drawn straight past the edge it was given.
    if let Some(mark) = overflow {
        let last = rows.last_mut().expect("one row at least");
        if cut || last.1 > wrap_width {
            // Each candidate is measured as `layout` will draw it: shaped
            // whole, mark included. Letters join and ligate, so what is left
            // of a word cannot be worked out from the widths of the letters
            // taken away. Reordering moves shaped runs without changing their
            // widths, so logical order measures the same.
            let drawn = |row: &str| width(&format!("{row}{mark}"));
            while drawn(&last.0) > wrap_width
                && let Some(at) = last.0.rfind(' ')
            {
                last.0.truncate(at);
            }
            // A single word with no space left to give up: letters go
            // instead, which is the only way the ellipsis can mean anything.
            // A piece is kept only once it has been measured to fit, so what
            // is drawn fits even where a shorter piece joins into a wider
            // form.
            if !last.0.is_empty() && drawn(&last.0) > wrap_width {
                let cuts = cuts(&last.0);
                let (mut fits, mut over) = (0, cuts.len());
                while over - fits > 1 {
                    let middle = (fits + over) / 2;
                    if drawn(&last.0[..cuts[middle]]) <= wrap_width {
                        fits = middle;
                    } else {
                        over = middle;
                    }
                }
                last.0.truncate(cuts[fits]);
            }
            last.0.push(mark);
        }
    }
    rows.into_iter().map(|(text, _)| text).collect()
}

/// Where `word` can be cut, as byte offsets from its start up to but not
/// including its end: never before a mark that rides on the letter ahead of
/// it, and never after a joiner.
fn cuts(word: &str) -> Vec<usize> {
    let mut previous = None;
    word.char_indices()
        .filter(|&(at, character)| {
            let joined = previous == Some('\u{200d}');
            previous = Some(character);
            let rides = matches!(bidi_class(character), BidiClass::NSM | BidiClass::BN);
            at == 0 || !(rides || joined)
        })
        .map(|(at, _)| at)
        .collect()
}

/// Where to paint a galley from [`layout`] so that it sits inside `rect`:
/// its left edge, or its right edge for right-to-left text.
pub fn galley_pos(rect: Rect, galley: &Galley) -> Pos2 {
    match galley.job.halign {
        Align::RIGHT => rect.right_top(),
        Align::Center => rect.center_top(),
        _ => rect.left_top(),
    }
}

/// Paints one line of text centred on `y`, starting at `left`, or ending at
/// `right` when it reads right to left. Returns the painted rect.
pub fn paint_line(
    painter: &Painter,
    left: f32,
    right: f32,
    y: f32,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    if !has_rtl(text) {
        return painter.text(pos2(left, y), Align2::LEFT_CENTER, text, font, color);
    }
    let galley = layout_line(painter, text, font, color);
    let rect = if is_rtl(text) {
        Align2::RIGHT_CENTER.anchor_size(pos2(right, y), galley.size())
    } else {
        Align2::LEFT_CENTER.anchor_size(pos2(left, y), galley.size())
    };
    painter.galley(rect.min, galley, color);
    rect
}
/// Puts each row of a galley laid out in logical order into visual order.
///
/// Upstream egui 0.37 keeps no per-glyph cluster or direction, which the
/// crmne fork used to move shaped runs. Without them a run cannot be moved
/// safely, so this leaves the galley in logical order: right-to-left text
/// still aligns and wraps from the right, but mixed-direction runs are not
/// reordered.
pub fn reorder(_galley: &mut Arc<Galley>) {}

/// A letter of a right-to-left script.
fn is_strong_rtl(character: char) -> bool {
    matches!(bidi_class(character), BidiClass::R | BidiClass::AL)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One unit of width per character, so a row's width is its length.
    fn by_character(piece: &str) -> f32 {
        piece.chars().count() as f32
    }

    #[test]
    fn a_word_longer_than_the_column_still_fits_the_column() {
        // A row has to hold something, so the first word of a row is taken
        // whatever it measures. `layout` then gives epaint no wrap width of
        // its own, so a row wider than the column is drawn past its edge.
        let one = break_rows("abcdefghij", 5.0, 1, Some(ELLIPSIS), by_character);
        assert_eq!(one, ["abcd\u{2026}"]);
        assert!(by_character(&one[0]) <= 5.0);

        // The same word with room for more rows: the last row is the one the
        // mark belongs to, and it is the one that has to fit.
        let two = break_rows("kl abcdefghij", 5.0, 2, Some(ELLIPSIS), by_character);
        assert!(by_character(two.last().unwrap()) <= 5.0);
        assert!(two.last().unwrap().ends_with(ELLIPSIS));

        // A column narrower than the mark itself still terminates.
        assert_eq!(
            break_rows("abc", 0.5, 1, Some(ELLIPSIS), by_character),
            ["\u{2026}"]
        );
    }

    #[test]
    fn rows_that_already_fit_are_left_alone() {
        // No mark where nothing was cut, which is what says a title is whole.
        assert_eq!(
            break_rows("ab cd ef gh", 5.0, 2, Some(ELLIPSIS), by_character),
            ["ab cd", "ef gh"]
        );
        // And the existing cut, which breaks at a space, is unchanged.
        assert_eq!(
            break_rows("ab cd ef gh", 5.0, 1, Some(ELLIPSIS), by_character),
            ["ab\u{2026}"]
        );
        // Without an overflow character nothing is added or removed.
        assert_eq!(
            break_rows("abcdefghij", 5.0, 1, None, by_character),
            ["abcdefghij"]
        );
    }

    /// The row `layout` draws for a right-to-left word longer than its
    /// column fits the column as epaint shapes it, in the fonts the app
    /// installs. Letters join and ligate, so no sum of letters measured on
    /// their own says how wide a piece of a word is; laying it out does.
    #[test]
    fn a_shaped_word_longer_than_the_column_is_drawn_inside_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // Fonts set on a context take effect from its next pass.
        ctx.run_ui(egui::RawInput::default(), |_| {})
            .textures_delta
            .clear();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let painter = ui.painter();
            let font = FontId::proportional(18.0);
            let measure = |piece: &str| {
                painter
                    .layout_no_wrap(piece.to_owned(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            };
            let mark = measure(&ELLIPSIS.to_string());
            for word in ["لالالالالالا", "והתקשרויותיהם"] {
                // Which Arabic and Hebrew faces a system has decides how wide
                // the word is, so the columns are taken from the word itself.
                let whole = measure(word);
                for share in [0.4, 0.6, 0.85] {
                    let wrap_width = (whole * share).max(mark + 1.0);
                    for max_rows in [1, 2] {
                        let galley = layout(
                            painter,
                            word,
                            font.clone(),
                            Color32::WHITE,
                            wrap_width,
                            max_rows,
                            Some(ELLIPSIS),
                        );
                        let text = galley.text();
                        assert_eq!(galley.rows.len(), 1, "{text:?}");
                        assert!(text.ends_with(ELLIPSIS), "{text:?}");
                        let drawn = galley.rows[0].size.x;
                        assert!(
                            drawn <= wrap_width,
                            "{word} in a {wrap_width} px column is drawn as {text}, {drawn} px wide"
                        );
                    }
                }
                // Given the room, the word stays whole and unmarked.
                let galley = layout(
                    painter,
                    word,
                    font.clone(),
                    Color32::WHITE,
                    whole + 1.0,
                    1,
                    Some(ELLIPSIS),
                );
                assert_eq!(galley.text(), word);
            }
        });
        output.textures_delta.clear();
    }

    #[test]
    fn detects_rtl() {
        assert!(is_rtl("غيوم في السماء"));
        assert!(is_rtl("מה נשמע"));
        assert!(is_rtl("السماء"));
        assert!(!is_rtl("Hello world"));
        assert!(!is_rtl("Hello غيوم")); // base LTR
        assert!(is_rtl("غيوم Hello")); // base RTL
        assert!(is_rtl("2024 غيوم")); // digits are weak
        assert!(!is_rtl(""));
        assert!(!is_rtl("   "));
        assert!(!is_rtl("123"));
        assert!(!is_rtl("Ça va"));
    }

    #[test]
    fn halign() {
        assert_eq!(halign_for("غيوم"), Align::RIGHT);
        assert_eq!(halign_for("Hello"), Align::LEFT);
    }

    /// Words fill rows up to the width, and a cut ends the last row.
    #[test]
    fn rows_fill_with_whole_words() {
        let width = |piece: &str| piece.chars().count() as f32 * 10.0;
        let text = "واحد اثنان ثلاثة";
        assert_eq!(
            break_rows(text, 110.0, usize::MAX, Some(ELLIPSIS), width),
            vec!["واحد اثنان".to_string(), "ثلاثة".to_string()]
        );
        assert_eq!(
            break_rows(text, 110.0, 1, Some(ELLIPSIS), width),
            vec!["واحد اثنان\u{2026}".to_string()]
        );
        // The mark has to fit too, so a word gives way to it.
        assert_eq!(
            break_rows(text, 100.0, 1, Some(ELLIPSIS), width),
            vec!["واحد\u{2026}".to_string()]
        );
        assert_eq!(
            break_rows("واحد\nاثنان", 200.0, usize::MAX, None, width),
            vec!["واحد".to_string(), "اثنان".to_string()]
        );
        assert_eq!(
            break_rows("واحد\nاثنان", 200.0, 1, Some(ELLIPSIS), width),
            vec!["واحد\u{2026}".to_string()]
        );
    }

}
