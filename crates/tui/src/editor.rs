use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy)]
pub(crate) enum Move {
    Left,
    Right,
    Home,
    End,
    Up,
    Down,
}

pub(crate) fn move_cursor(text: &str, cursor: usize, direction: Move) -> usize {
    let cursor = cursor.min(text.len());
    let start = text[..cursor].rfind('\n').map_or(0, |i| i + 1);
    let end = text[cursor..].find('\n').map_or(text.len(), |i| cursor + i);
    match direction {
        Move::Left => text[..cursor]
            .grapheme_indices(true)
            .next_back()
            .map_or(0, |(i, _)| i),
        Move::Right => text[cursor..]
            .graphemes(true)
            .next()
            .map_or(cursor, |g| cursor + g.len()),
        Move::Home => start,
        Move::End => end,
        Move::Up | Move::Down => {
            let column = text[start..cursor].graphemes(true).count();
            let (a, b) = if matches!(direction, Move::Up) {
                if start == 0 {
                    return cursor;
                }
                let b = start - 1;
                (text[..b].rfind('\n').map_or(0, |i| i + 1), b)
            } else {
                if end == text.len() {
                    return cursor;
                }
                let a = end + 1;
                (a, text[a..].find('\n').map_or(text.len(), |i| a + i))
            };
            a + text[a..b]
                .grapheme_indices(true)
                .nth(column)
                .map_or(b - a, |(i, _)| i)
        }
    }
}

pub(crate) fn layout(text: &str, cursor: usize, width: usize) -> (Vec<String>, (usize, usize)) {
    let width = width.max(2);
    let mut lines = vec![];
    let mut line = String::new();
    let mut cells = 0;
    let mut position = None;
    for (offset, g) in text.grapheme_indices(true) {
        if g != "\n" && cells + g.width() > width {
            lines.push(std::mem::take(&mut line));
            cells = 0;
        }
        if offset == cursor {
            position = Some((lines.len(), cells));
        }
        if g == "\n" {
            lines.push(std::mem::take(&mut line));
            cells = 0;
        } else {
            line.push_str(g);
            cells += g.width();
        }
    }
    if cursor == text.len() {
        if cells == width {
            lines.push(std::mem::take(&mut line));
            cells = 0;
        }
        position = Some((lines.len(), cells));
    }
    lines.push(line);
    (lines, position.unwrap_or((0, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn movement_respects_graphemes_and_logical_lines() {
        let s = "a界e\u{301}\nsecond\nz";
        let end = "a界e\u{301}".len();
        assert_eq!(move_cursor(s, end, Move::Left), "a界".len());
        assert_eq!(move_cursor(s, 1, Move::Right), "a界".len());
        assert_eq!(move_cursor(s, 1, Move::Down), end + 2);
        assert_eq!(move_cursor(s, end + 2, Move::Up), 1);
        assert_eq!(move_cursor(s, end + 2, Move::Home), end + 1);
        assert_eq!(move_cursor(s, end + 2, Move::End), end + 7);
    }
    #[test]
    fn visual_cursor_follows_wide_wrapping_and_trailing_newline() {
        assert_eq!(
            layout("a界b\n", 6, 3),
            (vec!["a界".into(), "b".into(), "".into()], (2, 0))
        );
        assert_eq!(layout("abc", 3, 3).1, (1, 0));
    }
}
