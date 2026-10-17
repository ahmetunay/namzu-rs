//! Terminal presentation only. The SDK, providers and durable journal stay outside.
mod editor;
mod model;
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
    fn emit(&mut self, event: Event) -> Result<(), AgentError> {
        self.0
            .send(UiMessage::Event(event))
            .map_err(|_| AgentError::Cancelled)
    }
    fn approve(&mut self, call: &ToolCall, context: &TurnContext) -> Result<bool, AgentError> {
        context.remaining()?;
        if matches!(
            call.name.as_str(),
            "read_file" | "list_files" | "search_files" | "diff_file"
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
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(io::Error::other(
            "TUI requires a terminal on stdin and stdout; use exec --json for pipes",
        ));
    }
    // Declaration order restores the terminal before joining a worker on unwind.
    let mut permission: Option<Permission> = None;
    let mut permission_scroll = 0usize;
    let mut active: Option<ActiveTurn> = None;
    let mut guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let runner = Arc::new(runner);
    let mut app = model::App::new(settings.session_id.clone());
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
            terminal.draw(|frame| {
                view::draw(frame, &app, &settings);
                if let Some(request) = &permission {
                    view::approval(frame, &request.call, permission_scroll);
                }
            })?;
            if !event::poll(Duration::from_millis(33))? {
                continue;
            }
            match event::read()? {
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
                            let runner = runner.clone();
                            let session_id = app.session_id.clone();
                            let (sender, receiver) = mpsc::sync_channel(64);
                            let worker = thread::Builder::new().name("namzu-turn".into()).spawn(
                                move || {
                                    runner(prompt, session_id, signal, &mut ChannelEvents(sender))
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
                InputEvent::Paste(text) => app.append_input(&text),
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
