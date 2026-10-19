use crate::execute::{RunOptions, config_for, execute_observed};
use clap::Args;
use namzu_core::Cancellation;
use namzu_core::Message;
use namzu_session::{PrivateStore, Session, SessionStatus, default_state_dir};
use namzu_tui::{Controls, History, HistoryEntry, Host, SessionSummary};
use std::sync::Mutex;

#[derive(Args)]
pub struct TuiArgs {
    #[command(flatten)]
    pub options: RunOptions,
    /// Offline scripted tool replay in a temporary workspace; no model/account.
    #[arg(long, conflicts_with = "resume")]
    pub demo: bool,
}

pub fn run(args: TuiArgs, cancellation: Cancellation) -> std::io::Result<()> {
    let settings = namzu_tui::Settings {
        title: if args.demo {
            "DEMO · scripted fixture, answer 12".into()
        } else {
            format!(
                "{} · {}",
                args.options
                    .provider
                    .as_deref()
                    .unwrap_or("configured provider"),
                args.options.model.as_deref().unwrap_or("configured model")
            )
        },
        workspace: if args.demo {
            "temporary demo workspace".into()
        } else {
            args.options.cwd.display().to_string()
        },
        permissions: if args.demo {
            "demo read/write".into()
        } else {
            format!(
                "read{}{} · {}",
                if args.options.allow_write {
                    "/write"
                } else {
                    "-only"
                },
                if args.options.allow_shell {
                    if args.options.computer.image.is_some() {
                        " + guest shell"
                    } else {
                        " + HOST SHELL"
                    }
                } else {
                    ""
                },
                args.options.computer.description()
            )
        },
        session_id: args.options.resume.clone(),
    };
    namzu_tui::run_host(
        settings,
        cancellation,
        AgentHost {
            options: Mutex::new(args.options),
            demo: args.demo,
        },
    )
}

struct AgentHost {
    options: Mutex<RunOptions>,
    demo: bool,
}
impl AgentHost {
    fn options(&self) -> Result<RunOptions, String> {
        self.options
            .lock()
            .map(|o| o.clone())
            .map_err(|_| "settings lock failed".into())
    }
    fn store(options: &RunOptions) -> Result<PrivateStore, String> {
        PrivateStore::open(
            options
                .state_dir
                .clone()
                .map_or_else(default_state_dir, Ok)
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }
    fn compatible(session: &Session, options: &RunOptions, c: &crate::config::Config) -> bool {
        let cwd = options.cwd.canonicalize().ok();
        let route: serde_json::Value = serde_json::from_str(&session.route).unwrap_or_default();
        if !options.mcp.servers.is_empty() || route.get("mcp").is_some() {
            let current = (|| {
                let ctx = namzu_core::TurnContext::new(
                    std::time::Duration::from_secs(5),
                    Cancellation::default(),
                )?;
                let state = options
                    .state_dir
                    .clone()
                    .map_or_else(default_state_dir, Ok)?;
                crate::mcp_runtime::identity(&state, &options.mcp, &ctx)
            })();
            if current.ok().as_ref() != route.get("mcp") {
                return false;
            }
        }
        if check_computer_route(options, &route).is_err() {
            return false;
        }
        let route = route.get("provider_route").unwrap_or(&route);
        session.status == SessionStatus::Complete
            && session.pending_call.is_none()
            && cwd.as_ref() == Some(&session.cwd)
            && if c.provider == "codex" {
                route["provider"] == "codex"
                    && route["model"].as_str() == c.model.as_deref()
                    && route["endpoint"] == "https://chatgpt.com/backend-api/codex/"
            } else {
                route == &serde_json::json!([c.provider, c.base_url, c.model])
            }
    }
}
fn check_computer_route(options: &RunOptions, route: &serde_json::Value) -> Result<(), ()> {
    let current = options.computer.spec().map_err(|_| ())?;
    let expected = route.get("computer");
    if current
        .as_ref()
        .map(|s| serde_json::to_value(s).unwrap())
        .as_ref()
        != expected
    {
        return Err(());
    }
    Ok(())
}
fn model_name(session: &Session) -> String {
    let route: serde_json::Value = serde_json::from_str(&session.route).unwrap_or_default();
    let route = route.get("provider_route").unwrap_or(&route);
    route
        .get("model")
        .or_else(|| route.get(2))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .chars()
        .take(256)
        .collect()
}
impl Host for AgentHost {
    fn turn(
        &self,
        prompt: String,
        session: Option<String>,
        cancellation: Cancellation,
        events: &mut dyn namzu_core::EventSink,
    ) -> namzu_tui::Outcome {
        if self.demo {
            return namzu_tui::Outcome {
                session_id: None,
                context_bytes: Some(0),
                error: crate::offline_demo_observed(cancellation, events).err(),
            };
        }
        let mut options = match self.options() {
            Ok(v) => v,
            Err(e) => {
                return namzu_tui::Outcome {
                    session_id: None,
                    context_bytes: None,
                    error: Some(e),
                };
            }
        };
        options.resume = session;
        let execution = execute_observed(prompt, options, cancellation, events);
        let context_bytes = execution
            .result
            .as_ref()
            .ok()
            .and_then(|turn| serde_json::to_vec(&turn.messages).ok())
            .map(|v| v.len());
        namzu_tui::Outcome {
            session_id: execution.session_id,
            context_bytes,
            error: execution.result.err().map(|e| e.to_string()),
        }
    }
    fn controls(&self) -> Result<Controls, String> {
        if self.demo {
            return Ok(Controls {
                provider: "demo".into(),
                model: "scripted".into(),
                ..Controls::default()
            });
        }
        let o = self.options()?;
        let c = config_for(&o, &Self::store(&o)?).map_err(|e| e.to_string())?;
        Ok(Controls {
            provider: c.provider,
            model: c.model.unwrap_or_default(),
            effort: o.effort.unwrap_or_default(),
            timeout: c.timeout,
            steps: c.max_steps,
            tool_calls: c.max_tool_calls,
            context_bytes: c.max_text_bytes,
        })
    }
    fn apply(&self, c: Controls) -> Result<(), String> {
        if self.demo {
            return Err("Demo uses a fixed scripted provider".into());
        }
        let mut o = self.options()?;
        let current = self.controls()?;
        if c.provider != current.provider {
            return Err("Provider changes require a new CLI invocation".into());
        }
        o.model = Some(c.model);
        o.effort = if c.effort.is_empty() {
            None
        } else {
            Some(c.effort)
        };
        o.timeout = Some(c.timeout);
        o.max_steps = Some(c.steps);
        o.max_tool_calls = Some(c.tool_calls);
        o.max_text_bytes = Some(c.context_bytes);
        config_for(&o, &Self::store(&o)?).map_err(|e| e.to_string())?;
        *self.options.lock().map_err(|_| "settings lock failed")? = o;
        Ok(())
    }
    fn sessions(&self) -> Result<Vec<SessionSummary>, String> {
        if self.demo {
            return Ok(vec![]);
        }
        let o = self.options()?;
        let store = Self::store(&o)?;
        let cwd = o.cwd.canonicalize().map_err(|_| "cannot open workspace")?;
        let mut sessions = vec![];
        for name in store
            .names()
            .map_err(|e| e.to_string())?
            .iter()
            .rev()
            .filter(|n| n.starts_with("s-") && n.ends_with(".json"))
            .take(256)
        {
            let s =
                Session::load(&store, name.trim_end_matches(".json")).map_err(|e| e.to_string())?;
            if s.cwd == cwd {
                sessions.push(SessionSummary {
                    model: model_name(&s),
                    status: match s.status {
                        SessionStatus::Complete => "complete",
                        SessionStatus::Failed => "failed",
                        SessionStatus::Running => "interrupted",
                    }
                    .into(),
                    id: s.id,
                    updated_ms: s.updated_ms,
                });
            }
        }
        sessions.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(sessions)
    }
    fn history(&self, id: &str) -> Result<History, String> {
        if self.demo {
            return Err("Demo has no persistent sessions".into());
        }
        let o = self.options()?;
        let store = Self::store(&o)?;
        let c = config_for(&o, &store).map_err(|e| e.to_string())?;
        let s = Session::load(&store, id).map_err(|e| e.to_string())?;
        if Some(&s.cwd) != o.cwd.canonicalize().ok().as_ref() {
            return Err("Session belongs to another workspace".into());
        }
        let resumable = Self::compatible(&s, &o, &c);
        let context_bytes = serde_json::to_vec(&s.messages)
            .map_err(|_| "cannot measure history")?
            .len();
        let mut entries = vec![];
        for message in s.messages {
            match message {
                Message::ToolImage {
                    tool_call_id,
                    image,
                } => entries.push(HistoryEntry {
                    label: "Image",
                    content: format!("Image {}×{} · F7 preview", image.width, image.height),
                    tool_id: Some(tool_call_id),
                    image: Some(image),
                }),
                Message::User { content } => entries.push(HistoryEntry {
                    image: None,
                    label: "You",
                    content,
                    tool_id: None,
                }),
                Message::Assistant { content, .. } if !content.is_empty() => {
                    entries.push(HistoryEntry {
                        image: None,
                        label: "Assistant",
                        content,
                        tool_id: None,
                    })
                }
                Message::Tool {
                    content,
                    tool_call_id,
                    is_error,
                } => entries.push(HistoryEntry {
                    image: None,
                    label: "Tool",
                    content: format!("{}\n{}", if is_error { "error" } else { "OK" }, content),
                    tool_id: Some(tool_call_id),
                }),
                _ => (), // Never send opaque replay/system data to the screen.
            }
        }
        Ok(History {
            id: s.id,
            entries,
            resumable,
            context_bytes,
            notice: if resumable {
                "Ready · account/route rechecked on send"
            } else {
                "View only · failed/interrupted or different model/route · Ctrl-N new"
            }
            .into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use serde_json::json;
    #[derive(Parser)]
    struct TestArgs {
        #[command(flatten)]
        options: RunOptions,
    }
    fn host(root: &std::path::Path) -> AgentHost {
        AgentHost {
            options: Mutex::new(
                TestArgs::parse_from([
                    "test",
                    "--cwd",
                    root.to_str().unwrap(),
                    "--state-dir",
                    root.join("state").to_str().unwrap(),
                    "--provider",
                    "codex",
                    "--model",
                    "fixture",
                ])
                .options,
            ),
            demo: false,
        }
    }
    #[test]
    fn settings_use_effective_config_and_apply_atomically_without_changing_permissions() {
        let root = tempfile::tempdir().unwrap();
        let host = host(root.path());
        let options = host.options().unwrap();
        let store = AgentHost::store(&options).unwrap();
        let config = crate::config::Config {
            timeout: 17,
            max_steps: 23,
            max_tool_calls: 3,
            max_text_bytes: 8192,
            ..Default::default()
        };
        store.write("config.json", &config).unwrap();
        let before = host.controls().unwrap();
        assert_eq!(
            (
                before.timeout,
                before.steps,
                before.tool_calls,
                before.context_bytes
            ),
            (17, 23, 3, 8192)
        );
        let mut invalid = before.clone();
        invalid.timeout = 0;
        assert!(host.apply(invalid).is_err());
        assert_eq!(host.controls().unwrap().timeout, 17);
        let mut next = before;
        next.model = "other-model".into();
        next.timeout = 20;
        next.context_bytes = 4096;
        host.apply(next).unwrap();
        let options = host.options().unwrap();
        assert!(!options.allow_shell && !options.allow_write && options.tool_allow.is_none());
        assert_eq!(host.controls().unwrap().model, "other-model");
        assert_eq!(config_for(&options, &store).unwrap().max_text_bytes, 4096);
    }
    #[test]
    fn private_history_filters_opaque_state_and_failed_or_different_routes_are_view_only() {
        let root = tempfile::tempdir().unwrap();
        let host = host(root.path());
        let store = AgentHost::store(&host.options().unwrap()).unwrap();
        let route = json!({"provider":"codex","model":"fixture","endpoint":"https://chatgpt.com/backend-api/codex/","account":"fixture-account"}).to_string();
        let mut session = Session::new(
            "s-history".into(),
            root.path().canonicalize().unwrap(),
            route,
        )
        .unwrap();
        session.messages = vec![
            Message::User {
                content: "old request".into(),
            },
            Message::ProviderState {
                data: json!({"opaque":"OPAQUE_MUST_STAY_PRIVATE"}),
            },
            Message::Assistant {
                content: "old answer".into(),
                tool_calls: vec![],
            },
        ];
        session.save(&store).unwrap();
        let history = host.history("s-history").unwrap();
        assert!(history.resumable);
        assert_eq!(history.entries.len(), 2);
        assert!(
            !history
                .entries
                .iter()
                .any(|e| e.content.contains("OPAQUE_MUST_STAY_PRIVATE"))
        );
        assert!(history.context_bytes > "old requestold answer".len());
        session.status = SessionStatus::Failed;
        session.save(&store).unwrap();
        assert!(!host.history("s-history").unwrap().resumable);
        session.status = SessionStatus::Complete;
        session.route = "[\"openai-compatible\",\"https://elsewhere.invalid\",\"fixture\"]".into();
        session.save(&store).unwrap();
        assert!(!host.history("s-history").unwrap().resumable);
        assert_eq!(host.sessions().unwrap().len(), 1);
        let other = tempfile::tempdir().unwrap();
        session.id = "s-other".into();
        session.cwd = other.path().canonicalize().unwrap();
        session.save(&store).unwrap();
        assert_eq!(host.sessions().unwrap().len(), 1);
        assert!(host.history("s-other").is_err());
    }
}
