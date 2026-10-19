use super::*;
use namzu_tui::{Controls, History, HistoryEntry, Host, SessionSummary};
use std::sync::Mutex;

pub(super) fn run(
    options: PalOptions,
    id: String,
    cancellation: Cancellation,
) -> std::io::Result<()> {
    // run_host checks the TTY before callbacks open any private state or contact a provider.
    namzu_tui::run_host(
        namzu_tui::Settings {
            title: format!("PAL · {id} · F5 tasks · F6 budgets"),
            workspace: "registry-owned Pal workspace; backend pinned in profile".into(),
            permissions: format!(
                "profile ceiling + current CLI: read{}{} · {}",
                if options.run.allow_write {
                    "/write"
                } else {
                    "-only"
                },
                if options.run.allow_shell {
                    " + shell"
                } else {
                    ""
                },
                options.run.computer.description()
            ),
            session_id: None,
        },
        cancellation,
        PalHost {
            options: Mutex::new(options),
            id,
        },
    )
}
struct PalHost {
    options: Mutex<PalOptions>,
    id: String,
}
impl PalHost {
    fn options(&self) -> Result<PalOptions, String> {
        self.options
            .lock()
            .map(|v| v.clone())
            .map_err(|_| "Pal options lock failed".into())
    }
    fn settings(&self) -> Result<(PalOptions, Registry, PalDefinition, Profile), String> {
        let o = self.options()?;
        o.config().map_err(|e| e.to_string())?;
        let r = o.registry().map_err(|e| e.to_string())?;
        let p = r.get(&o.owner, &self.id).map_err(|e| e.to_string())?;
        let profile = serde_json::from_str(&p.route).map_err(|_| "CLI Pal profile required")?;
        Ok((o, r, p, profile))
    }
}
impl Host for PalHost {
    fn turn(
        &self,
        prompt: String,
        session: Option<String>,
        cancellation: Cancellation,
        events: &mut dyn EventSink,
    ) -> namzu_tui::Outcome {
        let mut task = None;
        let result = (|| -> Result<Turn, AgentError> {
            let o = self
                .options()
                .map_err(|_| AgentError::State("Pal settings lock failed"))?;
            let (limits, ctx) = o.context(cancellation)?;
            let r = o.registry()?;
            let p = if let Some(source) = &session {
                task_definition(&r, &o, &self.id, source)?
            } else {
                r.get(&o.owner, &self.id)?
            };
            o.runtime(&p, &ctx)?;
            let id = new_id();
            queue(&r, &o, &self.id, &id, &prompt, session.as_deref())?;
            task = Some(id.clone());
            run_task(&r, &o, &self.id, &id, &ctx, limits, events)
        })();
        namzu_tui::Outcome {
            session_id: task,
            context_bytes: result
                .as_ref()
                .ok()
                .and_then(|t| serde_json::to_vec(&t.messages).ok())
                .map(|v| v.len()),
            error: result.err().map(|e| e.to_string()),
        }
    }
    fn controls(&self) -> Result<Controls, String> {
        let (o, _, _, p) = self.settings()?;
        let c = o.config().map_err(|e| e.to_string())?;
        Ok(Controls {
            provider: p.provider,
            model: p.model,
            effort: p.effort.unwrap_or_default(),
            timeout: c.timeout,
            steps: c.max_steps,
            tool_calls: c.max_tool_calls,
            context_bytes: c.max_text_bytes,
        })
    }
    fn apply(&self, c: Controls) -> Result<(), String> {
        let before = self.controls()?;
        if c.provider != before.provider || c.model != before.model || c.effort != before.effort {
            return Err("Pal model/account route is pinned; create a new Pal to change it".into());
        }
        let mut o = self.options()?;
        o.run.timeout = Some(c.timeout);
        o.run.max_steps = Some(c.steps);
        o.run.max_tool_calls = Some(c.tool_calls);
        o.run.max_text_bytes = Some(c.context_bytes);
        o.config().map_err(|e| e.to_string())?;
        *self.options.lock().map_err(|_| "Pal options lock failed")? = o;
        Ok(())
    }
    fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        let (o, r, _, p) = self.settings()?;
        let mut tasks = r.tasks(&o.owner, &self.id).map_err(|e| e.to_string())?;
        tasks.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(tasks
            .into_iter()
            .map(|t| SessionSummary {
                id: t.id,
                model: p.model.clone(),
                status: format!("{:?}", t.status).to_lowercase(),
                updated_ms: t.updated_ms,
            })
            .collect())
    }
    fn history(&self, id: &str) -> Result<History, String> {
        let (o, r, p, _) = self.settings()?;
        let task = r
            .tasks(&o.owner, &self.id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or("Pal task not found")?;
        let s = r
            .session(&o.owner, &self.id, id)
            .map_err(|e| e.to_string())?;
        let resumable = task.status == TaskStatus::Complete
            && s.pending_call.is_none()
            && s.status == namzu_session::SessionStatus::Complete
            && p.status == PalStatus::Active;
        let context_bytes = serde_json::to_vec(&s.messages)
            .map_err(|_| "cannot measure Pal history")?
            .len();
        let entries = s
            .messages
            .into_iter()
            .filter_map(|m| match m {
                Message::ToolImage {
                    tool_call_id,
                    image,
                } => Some(HistoryEntry {
                    label: "Image",
                    content: format!("Screenshot {}×{} · F7 preview", image.width, image.height),
                    tool_id: Some(tool_call_id),
                    image: Some(image),
                }),
                Message::User { content } => Some(HistoryEntry {
                    image: None,
                    label: "You",
                    content,
                    tool_id: None,
                }),
                Message::Assistant { content, .. } if !content.is_empty() => Some(HistoryEntry {
                    image: None,
                    label: "Assistant",
                    content,
                    tool_id: None,
                }),
                Message::Tool {
                    content,
                    tool_call_id,
                    ..
                } => Some(HistoryEntry {
                    image: None,
                    label: "Tool",
                    content,
                    tool_id: Some(tool_call_id),
                }),
                _ => None,
            })
            .collect();
        Ok(History {
            id: id.into(),
            entries,
            resumable,
            context_bytes,
            notice: if resumable {
                "Followup forks a new task; account rechecked on send"
            } else {
                "View only: queued/failed/interrupted/paused; Ctrl-N new task"
            }
            .into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    #[test]
    fn pal_history_is_scoped_and_filters_opaque_state_and_settings_cannot_escalate() {
        #[derive(Parser)]
        struct Parse {
            #[command(flatten)]
            options: PalOptions,
        }
        let root = tempfile::tempdir().unwrap();
        let o = Parse::parse_from([
            "test",
            "--state-dir",
            root.path().join("state").to_str().unwrap(),
        ])
        .options;
        let r = o.registry().unwrap();
        let p = r
            .create(PalCreate {
                owner: o.owner.clone(),
                name: "test".into(),
                purpose: String::new(),
                route: serde_json::to_string(&Profile {
                    version: 1,
                    provider: "openai-compatible".into(),
                    model: "fixture".into(),
                    base_url: "http://127.0.0.1:1/v1".into(),
                    api_key_env: "FIXTURE_KEY".into(),
                    effort: None,
                    identity: "fixture".into(),
                    guest: false,
                    mcp: Value::Null,
                })
                .unwrap(),
                tools: None,
            })
            .unwrap();
        r.queue_task(&o.owner, &p.id, "queued", "test").unwrap();
        let host = PalHost {
            options: Mutex::new(o),
            id: p.id,
        };
        assert!(!host.history("queued").unwrap().resumable);
        assert!(host.history("someone-else").is_err());
        assert_eq!(host.sessions().unwrap().len(), 1);
        let mut c = host.controls().unwrap();
        c.model = "different".into();
        assert!(host.apply(c).is_err());
        let mut c = host.controls().unwrap();
        c.timeout = 10;
        host.apply(c).unwrap();
        assert_eq!(host.controls().unwrap().timeout, 10);
        assert!(!host.options().unwrap().run.allow_write);
    }
}
