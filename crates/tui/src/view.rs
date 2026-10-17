use crate::{
    Settings,
    model::{App, visible},
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn wrapped(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = vec![];
    for line in text.split('\n') {
        let mut current = String::new();
        let mut cells = 0;
        for part in line.graphemes(true) {
            let n = part.width();
            if cells + n > width && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                cells = 0;
            }
            current.push_str(part);
            cells += n;
        }
        lines.push(current);
    }
    lines
}

#[cfg(test)]
fn input_tail(input: &str, width: usize) -> String {
    let mut parts = vec![];
    let mut cells = 0;
    for part in input.graphemes(true).rev() {
        if cells + part.width() > width {
            break;
        }
        cells += part.width();
        parts.push(part);
    }
    parts.reverse();
    parts.concat()
}

pub(crate) fn draw(frame: &mut Frame<'_>, app: &App, settings: &Settings) {
    let area = frame.area();
    if area.width < 16 || area.height < 10 {
        frame.render_widget(Paragraph::new("Terminal too small · Esc exits"), area);
        return;
    }
    let (input_lines, (cursor_row, cursor_col)) = crate::editor::layout(
        &app.input,
        app.cursor.min(app.input.len()),
        area.width.saturating_sub(4) as usize,
    );
    let input_height = input_lines
        .len()
        .clamp(1, 5)
        .min(area.height.saturating_sub(7).max(1) as usize);
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(input_height as u16 + 2),
        Constraint::Length(1),
    ])
    .split(area);
    frame.render_widget(
        Paragraph::new(format!(
            "Namzu Rust · {}\n{} · {}",
            visible(&settings.title, 256),
            visible(&settings.workspace, 256),
            visible(&settings.permissions, 128)
        ))
        .style(Style::default().fg(Color::Cyan)),
        areas[0],
    );
    let mut lines: Vec<Line<'static>> = vec![];
    let width = areas[1].width as usize;
    for entry in &app.entries {
        let color = match entry.label {
            "You" => Color::Cyan,
            "Assistant" => Color::Green,
            "Error" => Color::Red,
            _ => Color::Yellow,
        };
        let selected = entry.tool_id.is_some() && entry.tool_id == app.selected_tool;
        let label = if entry.tool_id.is_some() {
            format!(
                "{} {} {}",
                if selected { ">" } else { " " },
                entry.label,
                if entry.expanded { "[-]" } else { "[+]" }
            )
        } else {
            entry.label.into()
        };
        lines.push(Line::styled(label, Style::default().fg(color)));
        let count = if entry.tool_id.is_some() && !entry.expanded {
            3
        } else {
            usize::MAX
        };
        for (index, line) in entry.text.lines().enumerate() {
            if index >= count {
                lines.push(Line::raw("… Tab expands selected tool"));
                break;
            }
            let color = if entry.diff {
                if line.starts_with("@@") {
                    Color::Cyan
                } else if line.starts_with('+') {
                    Color::Green
                } else if line.starts_with('-') {
                    Color::Red
                } else {
                    Color::Reset
                }
            } else {
                Color::Reset
            };
            lines.extend(
                wrapped(line, width)
                    .into_iter()
                    .map(|line| Line::styled(line, Style::default().fg(color))),
            );
        }
        lines.push(Line::raw(""));
    }
    if !app.draft.is_empty() {
        lines.push(Line::styled(
            "Assistant · streaming (provisional)",
            Style::default().fg(Color::Green),
        ));
        lines.extend(wrapped(&app.draft, width).into_iter().map(Line::raw));
        if app.draft.len() >= 64 * 1024 - 4 {
            lines.push(Line::raw("[display limit]"));
        }
    }
    if lines.is_empty() {
        lines.push(Line::raw("Enter a message to begin."));
    }
    let height = areas[1].height as usize;
    let end = lines
        .len()
        .saturating_sub(app.scroll.min(lines.len().saturating_sub(height)));
    let start = end.saturating_sub(height);
    frame.render_widget(Paragraph::new(lines[start..end].to_vec()), areas[1]);
    let stats = app
        .stats
        .as_ref()
        .map(|s| {
            format!(
                " · {} model / {} tool calls",
                s.provider_calls, s.tool_calls
            )
        })
        .unwrap_or_default();
    let elapsed = if app.busy {
        format!(
            " · {}s",
            app.started.map(|i| i.elapsed().as_secs()).unwrap_or(0)
        )
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}{}\nSession: {}",
            visible(&app.status, 512),
            elapsed,
            stats,
            visible(app.session_id.as_deref().unwrap_or("new"), 128)
        )),
        areas[2],
    );
    let offset = cursor_row.saturating_sub(input_height - 1);
    let input = input_lines
        .iter()
        .skip(offset)
        .take(input_height)
        .enumerate()
        .map(|(i, line)| format!("{}{}", if i + offset == 0 { "> " } else { "  " }, line))
        .collect::<Vec<_>>()
        .join("\n");
    frame.render_widget(
        Paragraph::new(input).block(Block::default().borders(Borders::ALL).title(if app.busy {
            "Working"
        } else {
            "Message · F2/Alt-Enter newline"
        })),
        areas[3],
    );
    if !app.busy && !app.blocked {
        frame.set_cursor_position((
            areas[3].x + 3 + cursor_col as u16,
            areas[3].y + 1 + (cursor_row - offset) as u16,
        ));
    }
    frame.render_widget(
        Paragraph::new("Enter send · F2 newline · F3/F4 tool · Tab expand · PgUp/PgDn scroll · Ctrl-C cancel · Esc exit"),
        areas[4],
    );
}

pub(crate) fn approval(frame: &mut Frame<'_>, call: &namzu_core::ToolCall, scroll: usize) {
    use ratatui::{layout::Rect, widgets::Clear};
    let area = frame.area();
    let rect = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let text = format!(
        "Allow once? [y] yes / [n] deny\nEsc exits · Ctrl-C cancels · PgUp/PgDn details\n{}{}\n{}",
        visible(&call.name, 128),
        if call.name == "shell" {
            " · HOST filesystem/network access"
        } else {
            " · changes files"
        },
        visible(&call.arguments.to_string(), 64 * 1024)
    );
    let lines = wrapped(&text, rect.width.saturating_sub(2) as usize);
    let start = scroll.min(
        lines
            .len()
            .saturating_sub(rect.height.saturating_sub(2) as usize),
    );
    frame.render_widget(
        Paragraph::new(lines[start..].join("\n")).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Permission · one call only"),
        ),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use namzu_core::Event;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn unicode_wrap_and_input_tail_keep_graphemes_and_terminal_width() {
        assert_eq!(wrapped("a界b\ne\u{301}", 3), ["a界", "b", "e\u{301}"]);
        assert_eq!(input_tail("abc界e\u{301}", 3), "界e\u{301}");
    }
    #[test]
    fn real_widget_render_shows_provisional_then_committed_text_once() {
        let mut terminal = Terminal::new(TestBackend::new(70, 20)).unwrap();
        let settings = Settings {
            title: "fixture".into(),
            workspace: "/test".into(),
            permissions: "read-only".into(),
            session_id: None,
        };
        let mut app = App::new(None);
        app.append_input("hello");
        app.submit();
        app.event(Event::AssistantDelta {
            text: "partial".into(),
        });
        terminal.draw(|f| draw(f, &app, &settings)).unwrap();
        let snapshot = format!("{:?}", terminal.backend().buffer());
        assert!(snapshot.contains("provisional") && snapshot.contains("partial"));
        app.event(Event::AssistantMessage {
            text: "final answer".into(),
            has_tool_calls: false,
        });
        terminal.draw(|f| draw(f, &app, &settings)).unwrap();
        let snapshot = format!("{:?}", terminal.backend().buffer());
        assert!(!snapshot.contains("partial") && snapshot.contains("final answer"));
        assert_eq!(snapshot.matches("final answer").count(), 1);
    }
}
