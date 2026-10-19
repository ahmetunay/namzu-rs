//! Terminal presentation only. The SDK, providers and durable journal stay outside.
mod editor;
mod host;
mod model;
pub use host::{Controls, History, HistoryEntry, Host, SessionSummary};
mod view;
use crossterm::{
    cursor,
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste, Event as InputEvent, KeyCode,
        KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use namzu_core::{AgentError, Cancellation, Event, EventSink, ToolCall, TurnContext};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, IsTerminal},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

pub struct Settings {
    pub title: String,
    pub workspace: String,
    pub permissions: String,
    pub session_id: Option<String>,
}

pub struct Outcome {
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub context_bytes: Option<usize>,
}

enum UiMessage {
    Event(Event),
    Permission(Permission),
}
struct Permission {
    call: ToolCall,
    reply: mpsc::Sender<bool>,
}
struct ChannelEvents(SyncSender<UiMessage>);
impl EventSink for ChannelEvents {
    fn supports_approval(&self) -> bool {
        true
    }
    fn emit(&mut self, event: Event) -> Result<(), AgentError> {
        self.0
            .send(UiMessage::Event(event))
            .map_err(|_| AgentError::Cancelled)
    }
    fn approve(&mut self, call: &ToolCall, context: &TurnContext) -> Result<bool, AgentError> {
        context.remaining()?;
        if matches!(
            call.name.as_str(),
            "read_file"
                | "list_files"
                | "inspect_file"
                | "read_file_chunk"
                | "read_document"
                | "read_image"
                | "search_files"
                | "diff_file"
                | "computer_info"
                | "computer_screenshot"
                | "browser_info"
                | "browser_snapshot"
        ) {
            return Ok(true);
        }
        if call.arguments.to_string().len() > 64 * 1024 {
            return Ok(false);
        }
        let (reply, response) = mpsc::channel();
        self.0
            .send(UiMessage::Permission(Permission {
                call: call.clone(),
                reply,
            }))
            .map_err(|_| AgentError::Cancelled)?;
        loop {
            context.remaining()?;
            match response.recv_timeout(Duration::from_millis(10)) {
                Ok(value) => {
                    context.remaining()?;
                    return Ok(value);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(_) => return Err(AgentError::Cancelled),
            }
        }
    }
}

struct ActiveTurn {
    receiver: Option<Receiver<UiMessage>>,
    worker: Option<JoinHandle<Outcome>>,
    cancellation: Cancellation,
}
impl ActiveTurn {
    fn join(&mut self) -> Outcome {
        self.worker
            .take()
            .unwrap()
            .join()
            .unwrap_or_else(|_| Outcome {
                session_id: None,
                context_bytes: None,
                error: Some("agent worker stopped unexpectedly".into()),
            })
    }
}
impl Drop for ActiveTurn {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.cancellation.cancel();
            // Unblock a producer even if the terminal stopped consuming events.
            self.receiver.take();
            let _ = self.join();
        }
    }
}

struct TerminalGuard {
    restored: bool,
}
impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        let mut guard = Self { restored: false };
        if let Err(error) = execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            cursor::Hide
        ) {
            let _ = guard.restore();
            return Err(error);
        }
        Ok(guard)
    }
    fn restore(&mut self) -> io::Result<()> {
        if self.restored {
            return Ok(());
        }
        self.restored = true;
        let screen = execute!(
            io::stdout(),
            DisableBracketedPaste,
            LeaveAlternateScreen,
            cursor::Show
        );
        let raw = terminal::disable_raw_mode();
        screen.and(raw)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// One worker at a time. Escape restores the terminal, cancels and joins the
/// worker. HTTP awaits and permission waits observe cancellation without a detached worker.
/// Event delivery is bounded/backpressured and disconnected consumers stop work.
pub fn run(
    settings: Settings,
    shutdown: Cancellation,
    runner: impl Fn(String, Option<String>, Cancellation, &mut dyn EventSink) -> Outcome
    + Send
    + Sync
    + 'static,
) -> io::Result<()> {
    run_host(settings, shutdown, host::FunctionHost(runner))
}

pub fn run_host(
    mut settings: Settings,
    shutdown: Cancellation,
    host: impl Host + 'static,
) -> io::Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "TUI requires a terminal on stdin and stdout; use exec --json for pipes",
        ));
    }
    let mut controls = host.controls().map_err(io::Error::other)?;
    let mut session_menu: Option<Vec<SessionSummary>> = None;
    let mut session_selected = 0usize;
    let mut panel: Option<host::Panel> = None;
    // Declaration order restores the terminal before joining a worker on unwind.
    let mut permission: Option<Permission> = None;
    let mut permission_scroll = 0usize;
    let mut active: Option<ActiveTurn> = None;
    let mut guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let host = Arc::new(host);
    let mut app = model::App::new(settings.session_id.clone());
    if let Some(id) = &settings.session_id {
        app.load_history(host.history(id).map_err(io::Error::other)?);
    }
    let result = (|| {
        loop {
            if shutdown.is_cancelled() {
                break;
            }
            if let Some(turn) = active.as_mut() {
                for _ in 0..128 {
                    match turn.receiver.as_ref().unwrap().try_recv() {
                        Ok(UiMessage::Event(event)) => app.event(event),
                        Ok(UiMessage::Permission(request)) => {
                            permission = Some(request);
                            permission_scroll = 0;
                        }
                        Err(_) => break,
                    }
                }
                if turn.worker.as_ref().unwrap().is_finished() {
                    // Once joined, every send has completed; drain the final events.
                    let outcome = turn.join();
                    while let Ok(event) = turn.receiver.as_ref().unwrap().try_recv() {
                        if let UiMessage::Event(event) = event {
                            app.event(event);
                        }
                    }
                    permission = None;
                    app.finish(outcome);
                    active = None;
                }
            }
            app.context_limit = controls.context_bytes;
            if !controls.provider.is_empty() && controls.provider != "demo" {
                settings.title = format!(
                    "{} · {} · {}",
                    controls.provider, controls.model, controls.effort
                );
            }
            terminal.draw(|frame| {
                view::draw(frame, &app, &settings);
                if let Some(items) = &session_menu {
                    view::sessions(frame, items, session_selected);
                }
                if let Some(panel) = &panel {
                    view::settings(frame, panel);
                }
                if let Some(request) = &permission {
                    view::approval(frame, &request.call, permission_scroll);
                }
                if app.show_image {
                    view::image(frame, &app, permission.is_some());
                }
            })?;
            if !event::poll(Duration::from_millis(33))? {
                continue;
            }
            match event::read()? {
                InputEvent::Key(key)
                    if key.kind != KeyEventKind::Release && key.code == KeyCode::F(7) =>
                {
                    app.show_image = !app.show_image;
                }
                InputEvent::Key(key) if key.kind != KeyEventKind::Release && app.show_image => {
                    match key.code {
                        KeyCode::Esc => app.show_image = false,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(turn) = &active {
                                turn.cancellation.cancel();
                                app.show_image = false;
                                permission = None;
                            } else {
                                break;
                            }
                        }
                        _ => (),
                    }
                }
                InputEvent::Key(key)
                    if key.kind != KeyEventKind::Release && permission.is_some() =>
                {
                    match key.code {
                        KeyCode::Char('y' | 'n')
                            if !key.modifiers.contains(KeyModifiers::CONTROL) =>
                        {
                            let request = permission.take().unwrap();
                            let _ = request.reply.send(key.code == KeyCode::Char('y'));
                        }
                        KeyCode::PageUp => permission_scroll = permission_scroll.saturating_sub(8),
                        KeyCode::PageDown => {
                            permission_scroll = permission_scroll.saturating_add(8)
                        }
                        KeyCode::Esc => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            if let Some(turn) = &active {
                                turn.cancellation.cancel();
                            }
                            permission = None;
                            app.status = "Cancelling…".into();
                        }
                        _ => (),
                    }
                }
                InputEvent::Key(key)
                    if key.kind != KeyEventKind::Release && session_menu.is_some() =>
                {
                    let items = session_menu.as_ref().unwrap();
                    match key.code {
                        KeyCode::Esc | KeyCode::F(5) => session_menu = None,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Up => session_selected = session_selected.saturating_sub(1),
                        KeyCode::Down => {
                            session_selected =
                                (session_selected + 1).min(items.len().saturating_sub(1))
                        }
                        KeyCode::Enter => {
                            if let Some(item) = items.get(session_selected) {
                                match host.history(&item.id) {
                                    Ok(history) => app.load_history(history),
                                    Err(e) => app.status = e,
                                }
                            }
                            session_menu = None;
                        }
                        _ => (),
                    }
                }
                InputEvent::Key(key) if key.kind != KeyEventKind::Release && panel.is_some() => {
                    let p = panel.as_mut().unwrap();
                    match key.code {
                        KeyCode::Esc | KeyCode::F(6) => panel = None,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break;
                        }
                        KeyCode::Tab | KeyCode::Down => {
                            p.selected = (p.selected + 1) % p.fields.len()
                        }
                        KeyCode::Up | KeyCode::BackTab => {
                            p.selected = (p.selected + p.fields.len() - 1) % p.fields.len()
                        }
                        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            p.fields[p.selected].clear()
                        }
                        KeyCode::Backspace => {
                            let field = &mut p.fields[p.selected];
                            let cursor =
                                editor::move_cursor(field, field.len(), editor::Move::Left);
                            field.truncate(cursor);
                        }
                        KeyCode::Char(c)
                            if !key
                                .modifiers
                                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                        {
                            if p.fields[p.selected].len() + c.len_utf8() <= 256 {
                                p.fields[p.selected].push(c);
                            }
                        }
                        KeyCode::Enter => {
                            match p.parse().and_then(|next| {
                                host.apply(next.clone())?;
                                Ok(next)
                            }) {
                                Ok(next) => {
                                    controls = next;
                                    app.new_conversation();
                                    panel = None;
                                }
                                Err(e) => p.error = e,
                            }
                        }
                        _ => (),
                    }
                }
                InputEvent::Paste(text) if panel.is_some() => {
                    let p = panel.as_mut().unwrap();
                    let field = &mut p.fields[p.selected];
                    field.push_str(&model::visible(
                        &text.replace(['\n', '\r'], ""),
                        256usize.saturating_sub(field.len()),
                    ));
                }
                InputEvent::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        if let Some(turn) = &active {
                            turn.cancellation.cancel();
                            app.status = "Cancelling…".into();
                        } else {
                            break;
                        }
                    }
                    KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        app.new_conversation()
                    }
                    KeyCode::F(5) if active.is_none() => match host.sessions() {
                        Ok(items) => {
                            session_menu = Some(items);
                            session_selected = 0;
                        }
                        Err(e) => app.status = e,
                    },
                    KeyCode::F(6) if active.is_none() => {
                        panel = Some(host::Panel::new(controls.clone()))
                    }
                    KeyCode::PageUp => app.scroll = app.scroll.saturating_add(8),
                    KeyCode::PageDown => app.scroll = app.scroll.saturating_sub(8),
                    KeyCode::Backspace => app.backspace(),
                    KeyCode::Delete => app.delete(),
                    KeyCode::Left => app.move_input(editor::Move::Left),
                    KeyCode::Right => app.move_input(editor::Move::Right),
                    KeyCode::Home => app.move_input(editor::Move::Home),
                    KeyCode::End => app.move_input(editor::Move::End),
                    KeyCode::Up => app.move_input(editor::Move::Up),
                    KeyCode::Down => app.move_input(editor::Move::Down),
                    KeyCode::F(2) => app.append_input("\n"),
                    KeyCode::F(3) => app.select_tool(true),
                    KeyCode::F(4) => app.select_tool(false),
                    KeyCode::Tab => app.toggle_tool(),
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
                        app.append_input("\n")
                    }
                    KeyCode::Enter => {
                        if let Some(prompt) = app.submit() {
                            let cancellation = Cancellation::default();
                            let signal = cancellation.clone();
                            let host = host.clone();
                            let session_id = app.session_id.clone();
                            let (sender, receiver) = mpsc::sync_channel(64);
                            let worker = thread::Builder::new().name("namzu-turn".into()).spawn(
                                move || {
                                    host.turn(
                                        prompt,
                                        session_id,
                                        signal,
                                        &mut ChannelEvents(sender),
                                    )
                                },
                            )?;
                            active = Some(ActiveTurn {
                                receiver: Some(receiver),
                                worker: Some(worker),
                                cancellation,
                            });
                        }
                    }
                    KeyCode::Char(c)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        app.append_input(&c.to_string())
                    }
                    _ => (),
                },
                InputEvent::Paste(text) if session_menu.is_none() && permission.is_none() => {
                    app.append_input(&text)
                }
                InputEvent::Resize(_, _) => (),
                _ => (),
            }
        }
        Ok(())
    })();
    // No terminal modes are held while waiting for blocking I/O cancellation.
    let restored = guard.restore();
    drop(active);
    result.and(restored)
}

#[cfg(test)]
mod tests;
