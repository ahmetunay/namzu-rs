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
    let areas = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
        Constraint::Length(3),
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
        lines.push(Line::styled(entry.label, Style::default().fg(color)));
        lines.extend(wrapped(&entry.text, width).into_iter().map(Line::raw));
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
    frame.render_widget(
        Paragraph::new(format!(
            "{}{}\nSession: {}",
            visible(&app.status, 512),
            stats,
            visible(app.session_id.as_deref().unwrap_or("new"), 128)
        )),
        areas[2],
    );
    let input = input_tail(&app.input, areas[3].width.saturating_sub(5) as usize);
    frame.render_widget(
        Paragraph::new(format!("> {input}")).block(
            Block::default().borders(Borders::ALL).title(if app.busy {
                "Working"
            } else {
                "Message"
            }),
        ),
        areas[3],
    );
    if !app.busy && !app.blocked {
        frame.set_cursor_position((areas[3].x + 3 + input.width() as u16, areas[3].y + 1));
    }
    frame.render_widget(
        Paragraph::new("Enter send · Ctrl-C cancel · Ctrl-N new · PgUp/PgDn scroll · Esc exit"),
        areas[4],
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
