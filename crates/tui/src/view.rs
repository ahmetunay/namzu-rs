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

pub(crate) fn image(frame: &mut Frame<'_>, app: &App, pending: bool) {
    use ratatui::widgets::Clear;
    let area = frame.area();
    frame.render_widget(Clear, area);
    let title = if pending {
        "Image preview · F7/Esc back · approval pending"
    } else {
        "Image preview · F7/Esc back"
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(image) = &app.preview else {
        frame.render_widget(Paragraph::new("No image in this conversation"), inner);
        return;
    };
    if inner.width == 0 || inner.height < 2 {
        return;
    }
    frame.render_widget(
        Paragraph::new(format!(
            "{}×{} pixels · scaled terminal preview",
            image.width, image.height
        )),
        ratatui::layout::Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let w = (inner.width as usize).min(image.width);
    let h = ((inner.height - 1) as usize).min(image.height.div_ceil(2));
    for y in 0..h {
        for x in 0..w {
            let sx = x * image.width / w;
            let top = (2 * y) * image.height / (2 * h);
            let bottom = (2 * y + 1) * image.height / (2 * h);
            let a = (top * image.width + sx) * 3;
            let b = (bottom.min(image.height - 1) * image.width + sx) * 3;
            frame.buffer_mut()[(inner.x + x as u16, inner.y + 1 + y as u16)]
                .set_symbol("▀")
                .set_fg(Color::Rgb(image.rgb[a], image.rgb[a + 1], image.rgb[a + 2]))
                .set_bg(Color::Rgb(image.rgb[b], image.rgb[b + 1], image.rgb[b + 2]));
        }
    }
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
            let mut text = format!(
                " · {} model / {} tool calls",
                s.provider_calls, s.tool_calls
            );
            if !s.usage.is_empty() {
                let token = |value: &Option<namzu_core::provider::ReportedTotal>| {
                    value.as_ref().map_or_else(
                        || "?".into(),
                        |t| {
                            format!(
                                "{}{}",
                                t.sum,
                                if t.samples < s.usage.responses {
                                    "+"
                                } else {
                                    ""
                                }
                            )
                        },
                    )
                };
                text.push_str(&format!(
                    " · tokens {}/{} in/out",
                    token(&s.usage.input_tokens),
                    token(&s.usage.output_tokens)
                ));
            }
            text
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
            "{}{}{}\nSession: {} · history {}/{} B",
            visible(&app.status, 512),
            elapsed,
            stats,
            visible(app.session_id.as_deref().unwrap_or("new"), 128),
            app.context_used
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".into()),
            app.context_limit
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
        Paragraph::new("Enter send · F2 newline · Tab tool · F5 sessions · F6 settings · F7 image · Ctrl-C cancel · Esc exit"),
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
        match call.name.as_str() {
            "shell" => " · shell execution in selected host/guest backend",
            "audio_transcribe" | "audio_synthesize" =>
                " · sends audio/text to configured speech service",
            "audio_inspect" => " · inspect workspace audio locally",
            "lsp_apply" => " · apply hash-bound workspace edit",
            name if name.starts_with("lsp_") => " · run configured HOST LSP executable",
            name if name.starts_with("mcp_") => " · call selected MCP server",
            _ => " · workspace/computer operation",
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

pub(crate) fn sessions(frame: &mut Frame<'_>, items: &[crate::SessionSummary], selected: usize) {
    use ratatui::{layout::Rect, widgets::Clear};
    let area = frame.area();
    let rect = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let height = rect.height.saturating_sub(4) as usize;
    let offset = selected.saturating_sub(height.saturating_sub(1));
    let mut lines = vec![
        "Up/Down choose · Enter view/resume · Esc close".into(),
        "Only this workspace; failed/interrupted sessions are view-only.".into(),
    ];
    if items.is_empty() {
        lines.push("No saved sessions in this workspace".into());
    }
    for (i, item) in items.iter().enumerate().skip(offset).take(height) {
        lines.push(format!(
            "{} {} · {} · {}",
            if i == selected { ">" } else { " " },
            visible(&item.id, 128),
            visible(&item.model, 256),
            visible(&item.status, 32)
        ));
    }
    frame.render_widget(
        Paragraph::new(lines.join("\n")).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Sessions · private workspace history"),
        ),
        rect,
    );
}

pub(crate) fn settings(frame: &mut Frame<'_>, panel: &crate::host::Panel) {
    use ratatui::{layout::Rect, widgets::Clear};
    let area = frame.area();
    let rect = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Clear, rect);
    let labels = [
        "Model ID",
        "Effort (Codex only; empty=default)",
        "Timeout seconds",
        "Maximum model steps",
        "Maximum tool calls",
        "Conversation byte budget",
    ];
    let mut text = format!(
        "Tab/Up/Down field · Ctrl-U clears · Esc cancels\nEnter applies settings AND starts a new conversation.\nModel ID from `namzu-rs models`; byte budget is not a model token limit.\n{}\n",
        visible(&panel.error, 512)
    );
    for (i, (label, value)) in labels.iter().zip(&panel.fields).enumerate() {
        text.push_str(&format!(
            "{} {}: {}\n",
            if i == panel.selected { ">" } else { " " },
            label,
            visible(value, 256)
        ));
    }
    frame.render_widget(
        Paragraph::new(text)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Model and turn budgets · session-local"),
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
