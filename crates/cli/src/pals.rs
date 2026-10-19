//! CLI host adapter. Core/Pals never depend on this provider or presentation layer.
use crate::{
    config::Config,
    execute::{Driver, RunOptions, config_for, driver_for},
};
use clap::{Args, Subcommand};
use namzu_computer::{ComputerManager, ComputerSpec, Docker, GuestTools};
use namzu_core::{
    AgentError, Cancellation, EventSink, Limits, Message, Observer, Stats, ToolCall, Tools, Turn,
    TurnContext,
};
use namzu_pals::{
    BatchBudget, GuardedObserver, MailboxExecutor, PalCreate, PalDefinition, PalRun, PalStatus,
    PalUpdate, Registry, TaskStatus,
};
use namzu_session::{PrivateStore, default_state_dir, new_id};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf, time::Duration};
mod terminal;
mod transfer;

#[derive(Args)]
pub struct PalArgs {
    #[command(flatten)]
    pub options: PalOptions,
    #[command(subcommand)]
    action: Action,
}
#[derive(Args, Clone)]
pub struct PalOptions {
    #[command(flatten)]
    pub run: RunOptions,
    /// Owner namespace inside this OS user's private registry, not an OS account boundary.
    #[arg(long, default_value = "operator")]
    pub owner: String,
    /// Pin a new Pal to guest tools. Existing profiles always keep their backend.
    #[arg(long)]
    pub guest: bool,
    /// Explicit host control permission for owned-guest keyboard and mouse input.
    #[arg(long)]
    pub allow_input: bool,
    /// Permit explicit browser navigation for this invocation only.
    #[arg(long)]
    pub allow_navigate: bool,
    /// Permit explicit DOM input/selection/click actions for this invocation.
    #[arg(long)]
    pub allow_browser_actions: bool,
    /// Permit attaching one bounded guest file to a page input.
    #[arg(long)]
    pub allow_upload: bool,
    /// Permit a bounded browser download into a new guest workspace file.
    #[arg(long)]
    pub allow_download: bool,
    /// Explicit pinned local image: create a temporary computer for each turn.
    #[arg(long)]
    pub image: Option<String>,
    /// Trusted absolute Docker executable; only the local managed socket is used.
    #[arg(long, default_value = "/usr/local/bin/docker")]
    pub docker: PathBuf,
}
#[derive(Subcommand)]
enum Action {
    Create {
        name: String,
        #[arg(long, default_value = "")]
        purpose: String,
    },
    List,
    Show {
        id: String,
    },
    Update {
        id: String,
        #[arg(long)]
        revision: u64,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        purpose: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tools: Option<Vec<String>>,
    },
    Status {
        id: String,
        #[arg(long)]
        revision: u64,
        #[arg(value_parser=["active","paused","archived"])]
        status: String,
    },
    Tasks {
        id: String,
    },
    Inspect {
        id: String,
        task: String,
    },
    /// Queue a fresh task or explicitly fork a completed task's history.
    Queue {
        id: String,
        prompt: String,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        follow: Option<String>,
    },
    /// Run a queued task exactly once; interrupted/failed tasks cannot replay.
    Run {
        id: String,
        task: String,
    },
    Ask {
        id: String,
        prompt: String,
        #[arg(long)]
        follow: Option<String>,
    },
    /// Same prompt, separate persistent Pals, shared deadline and divided total budgets.
    Team {
        #[arg(required=true,num_args=1..)]
        ids: Vec<String>,
        #[arg(long)]
        prompt: String,
        #[arg(long,default_value_t=2,value_parser=clap::value_parser!(u32).range(1..=16))]
        concurrency: u32,
    },
    Send {
        id: String,
        message: String,
        body: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        in_reply_to: Option<String>,
    },
    Inbox {
        id: String,
    },
    /// Explicit bounded automatic FIFO pump; exits at idle/count/deadline, no daemon/retry.
    Work {
        id: String,
        #[arg(long,default_value_t=4,value_parser=clap::value_parser!(u32).range(1..=64))]
        max_messages: u32,
    },
    /// Close uncertain manual work after inspecting it; does not replay or undo effects.
    Acknowledge {
        id: String,
        task: String,
    },
    Resolve {
        id: String,
        message: String,
    },
    Computer {
        id: String,
        #[command(subcommand)]
        action: ComputerAction,
    },
    /// Interactive Pal conversation; F5 lists Pal tasks, F6 changes local budgets.
    Tui {
        id: String,
    },
}
#[derive(Subcommand)]
enum ComputerAction {
    Create {
        #[arg(long)]
        desktop: bool,
        #[arg(long, requires = "desktop")]
        browser: bool,
        #[arg(long, requires = "desktop")]
        width: Option<u16>,
        #[arg(long, requires = "desktop")]
        height: Option<u16>,
    },
    /// Execute a bounded desktop request from JSON; screenshot requires --output.
    Desktop {
        request: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Bounded Chromium info/navigation/DOM+AX request from JSON.
    Browser {
        request: PathBuf,
    },
    Show,
    Start,
    Stop,
    Restart,
    /// Inspect effects first. This acknowledges pending work without replay.
    Acknowledge,
    Upload {
        path: String,
        file: PathBuf,
    },
    Download {
        path: String,
        file: PathBuf,
    },
    List {
        #[arg(default_value = ".")]
        path: String,
    },
    /// Delete only this Pal's verified owned container and volume.
    Cleanup,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    version: u32,
    provider: String,
    model: String,
    base_url: String,
    api_key_env: String,
    effort: Option<String>,
    identity: String,
    guest: bool,
    #[serde(default)]
    mcp: serde_json::Value,
}
impl PalOptions {
    fn state(&self) -> Result<PathBuf, AgentError> {
        self.run
            .state_dir
            .clone()
            .map_or_else(default_state_dir, Ok)
    }
    fn store(&self) -> Result<PrivateStore, AgentError> {
        PrivateStore::open(self.state()?)
    }
    fn registry(&self) -> Result<Registry, AgentError> {
        let state = self.state()?;
        PrivateStore::open(&state)?;
        Registry::open(state.join("pals"), state.join("pal-workspaces"))
    }
    fn config(&self) -> Result<Config, AgentError> {
        if self.run.resume.is_some() {
            return Err(AgentError::Configuration(
                "Pal continuation uses --follow TASK, not --resume",
            ));
        }
        if self.run.cwd != std::path::Path::new(".") {
            return Err(AgentError::Configuration(
                "Pal workspaces are registry-owned; --cwd applies only to exec/tui",
            ));
        }
        if self.image.is_some() && self.run.computer.image.is_some() {
            return Err(AgentError::Configuration("select only one image flag"));
        }
        if let Some(image) = self.image.as_ref().or(self.run.computer.image.as_ref()) {
            self.run.computer.spec_for(image)?;
        } else {
            self.run.computer.spec()?;
        }
        config_for(&self.run, &self.store()?)
    }
    fn context(&self, cancellation: Cancellation) -> Result<(Limits, TurnContext), AgentError> {
        let c = self.config()?;
        let limits = Limits {
            timeout: Duration::from_secs(c.timeout),
            max_steps: c.max_steps,
            max_tool_calls: c.max_tool_calls,
            max_text_bytes: c.max_text_bytes,
        };
        let context = TurnContext::new(limits.timeout, cancellation)?;
        Ok((limits, context))
    }
    fn new_profile(&self, ctx: &TurnContext) -> Result<Profile, AgentError> {
        let c = self.config()?;
        let driver = driver_for(&self.run, &c)?;
        if self.run.computer.policy().active()
            && !(self.guest || self.image.is_some() || self.run.computer.image.is_some())
        {
            return Err(AgentError::Configuration(
                "visual Pal ceiling requires --guest or explicit computer image",
            ));
        }
        Ok(Profile {
            version: 1,
            provider: c.provider.clone(),
            model: c
                .model
                .clone()
                .ok_or(AgentError::Configuration("model required"))?,
            base_url: c.base_url.clone(),
            api_key_env: c.api_key_env.clone(),
            effort: self.run.effort.clone(),
            identity: driver.route(&c, ctx)?,
            guest: self.guest || self.image.is_some() || self.run.computer.image.is_some(),
            mcp: crate::mcp_runtime::identity(&self.state()?, &self.run.mcp, ctx)?,
        })
    }
    fn runtime(
        &self,
        pal: &PalDefinition,
        ctx: &TurnContext,
    ) -> Result<(Driver, Config, Profile), AgentError> {
        let p: Profile = serde_json::from_str(&pal.route).map_err(|_| {
            AgentError::Configuration("Pal route was not created by this CLI version")
        })?;
        if p.version != 1
            || (self.guest || self.image.is_some() || self.run.computer.image.is_some()) && !p.guest
        {
            return Err(AgentError::Configuration(
                "Pal backend/version mismatch; create a new Pal explicitly",
            ));
        }
        if self.run.computer.policy().active()
            && (!p.guest
                || !pal
                    .tools
                    .iter()
                    .any(|name| namzu_computer::VISUAL_NAMES.contains(&name.as_str())))
        {
            return Err(AgentError::Configuration(
                "visual tools require a guest Pal created with an explicit visual ceiling",
            ));
        }
        self.mcp_selection(&p, ctx)?;
        let mut o = self.run.clone();
        for (arg, saved) in [
            (&mut o.provider, &p.provider),
            (&mut o.model, &p.model),
            (&mut o.api_key_env, &p.api_key_env),
        ] {
            if arg.as_ref().is_some_and(|v| v != saved) {
                return Err(AgentError::Configuration(
                    "explicit provider/model/key selector differs from pinned Pal route",
                ));
            }
            *arg = Some(saved.clone());
        }
        if o.base_url.as_ref().is_some_and(|v| v != &p.base_url)
            || o.effort
                .as_ref()
                .is_some_and(|v| Some(v) != p.effort.as_ref())
        {
            return Err(AgentError::Configuration(
                "endpoint/effort differs from pinned Pal route",
            ));
        }
        if p.provider == "codex" && o.base_url.is_some() {
            return Err(AgentError::Configuration("Codex uses its fixed endpoint"));
        }
        o.base_url = if p.provider == "codex" {
            None
        } else {
            Some(p.base_url.clone())
        };
        o.effort = p.effort.clone();
        let c = config_for(&o, &self.store()?)?;
        let driver = driver_for(&o, &c)?;
        if driver.route(&c, ctx)? != p.identity {
            return Err(AgentError::State(
                "Pal account/provider route changed; execution refused",
            ));
        }
        Ok((driver, c, p))
    }
    fn mcp_selection(
        &self,
        p: &Profile,
        ctx: &TurnContext,
    ) -> Result<crate::mcp_runtime::Selection, AgentError> {
        let selected = if p.mcp.is_null() {
            crate::mcp_runtime::Selection::default()
        } else {
            serde_json::from_value(p.mcp["selection"].clone())
                .map_err(|_| AgentError::Configuration("invalid Pal MCP selection"))?
        };
        if !self.run.mcp.servers.is_empty() && self.run.mcp != selected {
            return Err(AgentError::Configuration(
                "MCP selection differs from pinned Pal profile",
            ));
        }
        if crate::mcp_runtime::identity(&self.state()?, &selected, ctx)? != p.mcp {
            return Err(AgentError::State(
                "Pal MCP configuration changed; create a new profile",
            ));
        }
        Ok(selected)
    }
    fn manager(&self, registry: &Registry, id: &str) -> Result<ComputerManager, AgentError> {
        registry.computers(&self.owner, id, Docker::local(&self.docker)?)
    }
}
fn policy(o: &RunOptions, c: &Config) -> ToolPolicy {
    let allow = match (&c.tool_allow, &o.tool_allow) {
        (Some(a), Some(b)) => Some(
            a.intersection(&b.iter().cloned().collect())
                .cloned()
                .collect(),
        ),
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.iter().cloned().collect()),
        _ => None,
    };
    let mut deny = c.tool_deny.clone();
    deny.extend(o.tool_deny.iter().cloned());
    ToolPolicy {
        allow_write: o.allow_write,
        allow_shell: o.allow_shell,
        allow,
        deny,
    }
}
/// Forward approval as well as events; the Pal run supplies its own durable journal.
struct Presentation<'a>(&'a mut dyn EventSink);
impl Observer for Presentation<'_> {
    fn supports_approval(&self) -> bool {
        self.0.supports_approval()
    }
    fn approve(&mut self, c: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        self.0.approve(c, ctx)
    }
    fn checkpoint(
        &mut self,
        _: &[Message],
        _: &Stats,
        _: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        Ok(())
    }
    fn event(&mut self, e: namzu_core::Event) -> Result<(), AgentError> {
        self.0.emit(e)
    }
}
struct ObserverSink<'a, O>(&'a mut O);
impl<O: Observer> EventSink for ObserverSink<'_, O> {
    fn supports_approval(&self) -> bool {
        self.0.supports_approval()
    }
    fn approve(&mut self, c: &ToolCall, ctx: &TurnContext) -> Result<bool, AgentError> {
        self.0.approve(c, ctx)
    }
    fn emit(&mut self, e: namzu_core::Event) -> Result<(), AgentError> {
        self.0.event(e)
    }
}
fn execute_run(
    o: &PalOptions,
    registry: &Registry,
    run: PalRun<'_>,
    ctx: &TurnContext,
    limits: Limits,
    observer: &mut impl Observer,
) -> Result<Turn, AgentError> {
    let pal = run.definition().clone();
    let (mut driver, c, profile) = o.runtime(&pal, ctx)?;
    let tool_policy = policy(&o.run, &c);
    if !profile.guest {
        let mut tools = TerminalTools::new(&pal.workspace, tool_policy.clone())
            .map_err(|_| AgentError::State("cannot open Pal tools"))?;
        let selection = o.mcp_selection(&profile, ctx)?;
        if selection.servers.is_empty() || !pal.tools.iter().any(|s| s.starts_with("mcp_")) {
            return run.execute(&mut driver, &mut tools, ctx, limits, observer);
        }
        let registry_mcp =
            namzu_mcp::config::Registry::open(&o.state()?).map_err(crate::mcp_runtime::error)?;
        let manager = crate::mcp_runtime::manager(&registry_mcp, &selection, ctx)?;
        let approvals = namzu_mcp::agent::Approvals::default();
        let current = policy(&o.run, &c);
        let admitted = match current.allow {
            Some(a) => pal.tools.intersection(&a).cloned().collect(),
            None => pal.tools.clone(),
        };
        let mut tools = crate::mcp_runtime::load(
            tools,
            manager,
            crate::mcp_runtime::Admission {
                selection: &selection,
                allow: Some(&admitted),
                deny: &current.deny,
                explicit: o.run.mcp_approve,
            },
            approvals.clone(),
            &mut ObserverSink(observer),
            ctx,
        )?;
        let explicit = o.run.mcp_approve || observer.supports_approval();
        let result = run.execute(
            &mut driver,
            &mut tools,
            ctx,
            limits,
            &mut namzu_mcp::agent::ApprovedObserver {
                inner: observer,
                approvals,
                explicit,
            },
        );
        let cleanup = TurnContext::new(Duration::from_secs(5), Cancellation::default())?;
        let closed = tools.close(&cleanup).map_err(crate::mcp_runtime::error);
        return match result {
            Err(e) => Err(e),
            Ok(turn) => {
                closed?;
                Ok(turn)
            }
        };
    }
    let manager = o.manager(registry, &pal.id)?;
    let ephemeral = o.image.as_ref().or(o.run.computer.image.as_ref());
    let mut computer = if let Some(image) = ephemeral {
        manager.acquire(o.run.computer.spec_for(image)?, ctx)?
    } else {
        manager.resume(ctx)?
    };
    if ephemeral.is_none() {
        computer.preserve_workspace_on_drop();
    }
    // Pending guest work needs explicit host inspection; never acknowledge on behalf of a model.
    if computer.snapshot().pending.is_some() {
        return Err(AgentError::State(
            "uncertain computer work; inspect and acknowledge explicitly",
        ));
    }
    start_if_stopped(&mut computer, ctx)?;
    let selection = o.mcp_selection(&profile, ctx)?;
    let mut tools =
        GuestTools::new(computer, tool_policy.clone()).with_visual(o.run.computer.policy());
    if !selection.servers.is_empty() && pal.tools.iter().any(|s| s.starts_with("mcp_")) {
        #[cfg(not(unix))]
        return Err(AgentError::Configuration("guest MCP requires Unix"));
        #[cfg(unix)]
        {
            let shared = namzu_computer::SharedGuestTools::new(tools);
            let registry_mcp = namzu_mcp::config::Registry::open(&o.state()?)
                .map_err(crate::mcp_runtime::error)?;
            let manager = crate::mcp_runtime::manager(&registry_mcp, &selection, ctx)?
                .with_guest(shared.launcher());
            let approvals = namzu_mcp::agent::Approvals::default();
            let admitted = match &tool_policy.allow {
                Some(a) => pal.tools.intersection(a).cloned().collect(),
                None => pal.tools.clone(),
            };
            let mut agent = crate::mcp_runtime::load(
                shared.clone(),
                manager,
                crate::mcp_runtime::Admission {
                    selection: &selection,
                    allow: Some(&admitted),
                    deny: &tool_policy.deny,
                    explicit: o.run.mcp_approve,
                },
                approvals.clone(),
                &mut ObserverSink(observer),
                ctx,
            )?;
            let explicit = o.run.mcp_approve || observer.supports_approval();
            let result = run.execute(
                &mut driver,
                &mut agent,
                ctx,
                limits,
                &mut namzu_mcp::agent::ApprovedObserver {
                    inner: observer,
                    approvals,
                    explicit,
                },
            );
            let cleanup = TurnContext::new(Duration::from_secs(20), Cancellation::default())?;
            let closed = agent.close(&cleanup).map_err(crate::mcp_runtime::error);
            drop(agent);
            shared.finish_mcp(result.is_ok() && closed.is_ok(), &cleanup)?;
            let mut computer = shared.into_computer()?;
            if ephemeral.is_none() || computer.snapshot().pending.is_some() {
                computer.detach(&cleanup)?;
            } else {
                computer.close(&cleanup)?;
            }
            return match result {
                Err(e) => Err(e),
                Ok(turn) => {
                    closed?;
                    Ok(turn)
                }
            };
        }
    }
    let result = run.execute(&mut driver, &mut tools, ctx, limits, observer);
    let cleanup = TurnContext::new(Duration::from_secs(20), Cancellation::default())?;
    let mut computer = tools.into_computer();
    if ephemeral.is_none() || computer.snapshot().pending.is_some() {
        // Preserve persistent/uncertain work, but stop it even after parent cancellation.
        if result.is_ok()
            && computer.snapshot().pending.is_none()
            && computer.snapshot().spec.desktop.is_some()
        {
            computer.release(&cleanup)?;
        } else {
            computer.detach(&cleanup)?;
        }
    } else {
        computer.close(&cleanup)?;
    }
    result
}
struct Worker<'a> {
    options: &'a PalOptions,
    registry: &'a Registry,
    events: &'a mut dyn EventSink,
}
impl MailboxExecutor for Worker<'_> {
    fn execute(
        &mut self,
        run: PalRun<'_>,
        ctx: &TurnContext,
        limits: Limits,
        guard: &mut dyn Observer,
    ) -> Result<Turn, AgentError> {
        execute_run(
            self.options,
            self.registry,
            run,
            ctx,
            limits,
            &mut GuardedObserver {
                host: &mut Presentation(self.events),
                guard,
            },
        )
    }
}
fn task_definition(
    registry: &Registry,
    o: &PalOptions,
    id: &str,
    task: &str,
) -> Result<PalDefinition, AgentError> {
    let t = registry
        .tasks(&o.owner, id)?
        .into_iter()
        .find(|t| t.id == task)
        .ok_or(AgentError::State("Pal task not found"))?;
    registry.revision(&o.owner, id, t.revision)
}
fn run_task(
    registry: &Registry,
    o: &PalOptions,
    id: &str,
    task: &str,
    ctx: &TurnContext,
    limits: Limits,
    events: &mut dyn EventSink,
) -> Result<Turn, AgentError> {
    // Route/account validation precedes changing a queued task's state.
    o.runtime(&task_definition(registry, o, id, task)?, ctx)?;
    let run = registry.start_task(&o.owner, id, task)?;
    execute_run(o, registry, run, ctx, limits, &mut Presentation(events))
}
fn queue(
    registry: &Registry,
    o: &PalOptions,
    id: &str,
    task: &str,
    prompt: &str,
    follow: Option<&str>,
) -> Result<namzu_pals::TaskRecord, AgentError> {
    if let Some(source) = follow {
        registry.queue_followup(&o.owner, id, task, source, prompt)
    } else {
        registry.queue_task(&o.owner, id, task, prompt)
    }
}
fn report(id: &str, task: &str, result: Result<Turn, AgentError>) -> Value {
    match result {
        Ok(t) => {
            json!({"pal_id":id,"task_id":task,"status":"ok","output":t.output,"stats":t.stats})
        }
        Err(e) => json!({"pal_id":id,"task_id":task,"status":"error","error":e.to_string()}),
    }
}
fn allocation(total: &Limits, count: usize) -> Result<Limits, AgentError> {
    if !(1..=64).contains(&count) || total.max_steps < count || total.max_text_bytes < count {
        return Err(AgentError::Configuration(
            "batch count exceeds total steps/context allocation",
        ));
    }
    Ok(Limits {
        timeout: total.timeout,
        max_steps: total.max_steps / count,
        max_tool_calls: total.max_tool_calls / count,
        max_text_bytes: total.max_text_bytes / count,
    })
}
pub fn run(args: PalArgs, cancellation: Cancellation) -> Result<bool, AgentError> {
    if let Action::Tui { id } = &args.action {
        terminal::run(args.options, id.clone(), cancellation)
            .map_err(|_| AgentError::State("Pal TUI failed; terminal required"))?;
        return Ok(true);
    }
    let o = &args.options;
    let (limits, ctx) = o.context(cancellation)?;
    let registry = o.registry()?;
    let value = match args.action {
        Action::Create { name, purpose } => {
            let p = o.new_profile(&ctx)?;
            let c = o.config()?;
            let policy = policy(&o.run, &c);
            let mut tools: BTreeSet<String> = if p.guest {
                ["read_file", "list_files", "write_file", "shell"]
                    .into_iter()
                    .filter(|n| {
                        !policy.deny.contains(*n)
                            && policy.allow.as_ref().is_none_or(|a| a.contains(*n))
                            && match *n {
                                "write_file" => policy.allow_write,
                                "shell" => policy.allow_shell,
                                _ => true,
                            }
                    })
                    .map(String::from)
                    .collect()
            } else {
                TerminalTools::new(o.state()?, policy.clone())
                    .map_err(|_| AgentError::State("cannot open tool catalogue"))?
                    .definitions()
                    .into_iter()
                    .map(|t| t.name)
                    .collect()
            };
            if p.guest {
                let v = o.run.computer.policy();
                tools.extend(
                    namzu_computer::VISUAL_NAMES
                        .iter()
                        .filter(|name| {
                            !policy.deny.contains(**name)
                                && policy.allow.as_ref().is_none_or(|a| a.contains(**name))
                                && match **name {
                                    "computer_info"
                                    | "computer_screenshot"
                                    | "browser_info"
                                    | "browser_snapshot" => v.observe,
                                    "computer_input" => v.input,
                                    "browser_navigate" => v.navigate,
                                    "browser_action" => v.actions,
                                    "browser_upload" => v.upload,
                                    "browser_download" => v.download,
                                    _ => false,
                                }
                        })
                        .map(|s| (*s).to_string()),
                );
            }
            if !p.mcp.is_null()
                && let Some(allow) = &o.run.tool_allow
            {
                tools.extend(
                    allow
                        .iter()
                        .filter(|s| {
                            s.starts_with("mcp_")
                                && !c.tool_deny.contains(*s)
                                && !o.run.tool_deny.contains(*s)
                        })
                        .cloned(),
                );
            }
            json!(
                registry.create(PalCreate {
                    owner: o.owner.clone(),
                    name,
                    purpose,
                    route: serde_json::to_string(&p)
                        .map_err(|_| AgentError::State("cannot encode Pal profile"))?,
                    tools: Some(tools)
                })?
            )
        }
        Action::List => json!(registry.list(&o.owner)?),
        Action::Show { id } => json!(registry.get(&o.owner, &id)?),
        Action::Update {
            id,
            revision,
            name,
            purpose,
            tools,
        } => json!(registry.update(
            &o.owner,
            &id,
            revision,
            PalUpdate {
                name,
                purpose,
                tools: tools.map(|t| t.into_iter().collect()),
                ..Default::default()
            }
        )?),
        Action::Status {
            id,
            revision,
            status,
        } => json!(registry.update(
            &o.owner,
            &id,
            revision,
            PalUpdate {
                status: Some(match status.as_str() {
                    "active" => PalStatus::Active,
                    "paused" => PalStatus::Paused,
                    _ => PalStatus::Archived,
                }),
                ..Default::default()
            }
        )?),
        Action::Tasks { id } => json!(registry.tasks(&o.owner, &id)?),
        Action::Inspect { id, task } => json!(registry.session(&o.owner, &id, &task)?),
        Action::Queue {
            id,
            prompt,
            task,
            follow,
        } => json!(queue(
            &registry,
            o,
            &id,
            &task.unwrap_or_else(new_id),
            &prompt,
            follow.as_deref()
        )?),
        Action::Ask { id, prompt, follow } => {
            let task = new_id();
            let pal = if let Some(source) = &follow {
                task_definition(&registry, o, &id, source)?
            } else {
                registry.get(&o.owner, &id)?
            };
            o.runtime(&pal, &ctx)?;
            queue(&registry, o, &id, &task, &prompt, follow.as_deref())?;
            report(
                &id,
                &task,
                run_task(
                    &registry,
                    o,
                    &id,
                    &task,
                    &ctx,
                    limits,
                    &mut namzu_core::NoopEvents,
                ),
            )
        }
        Action::Run { id, task } => report(
            &id,
            &task,
            run_task(
                &registry,
                o,
                &id,
                &task,
                &ctx,
                limits,
                &mut namzu_core::NoopEvents,
            ),
        ),
        Action::Team {
            ids,
            prompt,
            concurrency,
        } => team(
            &registry,
            o,
            &ids,
            &prompt,
            concurrency as usize,
            &ctx,
            &limits,
        )?,
        Action::Send {
            id,
            message,
            body,
            from,
            in_reply_to,
        } => {
            let receipt = registry
                .mailbox(&o.owner, &id)?
                .send(namzu_pals::MailMessage {
                    id: message,
                    body,
                    sender: from.map_or(namzu_pals::MailSender::Operator, |id| {
                        namzu_pals::MailSender::Pal { id }
                    }),
                    in_reply_to,
                })?;
            json!({"task_id":receipt.task_id,"inserted":receipt.inserted})
        }
        Action::Inbox { id } => json!(registry.mailbox(&o.owner, &id)?.messages()?),
        Action::Work { id, max_messages } => {
            let per = allocation(&limits, max_messages as usize)?;
            let mut events = namzu_core::NoopEvents;
            let result = registry.mailbox(&o.owner, &id)?.drain(
                &new_id(),
                &ctx,
                BatchBudget {
                    max_messages: max_messages as usize,
                    per_message: per,
                    total: limits,
                },
                &mut Worker {
                    options: o,
                    registry: &registry,
                    events: &mut events,
                },
            )?;
            let deliveries:Vec<_>=result.deliveries.into_iter().map(|d|json!({"message_id":d.message_id,"delivery":d.delivery,"result":report(&id,&d.task_id,d.result)})).collect();
            json!({"deliveries":deliveries,"stopped_error":result.stopped.map(|e|e.to_string())})
        }
        Action::Acknowledge { id, task } => {
            registry.acknowledge_interruption(&o.owner, &id, &task)?;
            json!({"acknowledged":true})
        }
        Action::Resolve { id, message } => {
            registry
                .mailbox(&o.owner, &id)?
                .resolve_inspection(&message)?;
            json!({"resolved":true})
        }
        Action::Computer { id, action } => computer(o, &registry, &id, action, &ctx)?,
        Action::Tui { .. } => unreachable!(),
    };
    let success = value.get("status").and_then(Value::as_str) != Some("error")
        && value
            .get("results")
            .and_then(Value::as_array)
            .is_none_or(|v| v.iter().all(|r| r["status"] == "ok"))
        && value.get("stopped_error").is_none_or(Value::is_null)
        && value
            .get("deliveries")
            .and_then(Value::as_array)
            .is_none_or(|v| v.iter().all(|r| r["result"]["status"] == "ok"));
    println!(
        "{}",
        json!({"format_version":1,"status":if success{"ok"}else{"error"},"result":value})
    );
    Ok(success)
}
fn team(
    registry: &Registry,
    o: &PalOptions,
    ids: &[String],
    prompt: &str,
    concurrency: usize,
    ctx: &TurnContext,
    total: &Limits,
) -> Result<Value, AgentError> {
    let per = allocation(total, ids.len())?;
    if prompt.is_empty()
        || prompt.len() > 16 * 1024
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AgentError::Configuration(
            "unique Pals and bounded prompt required",
        ));
    }
    // Validate the entire batch before publishing any task. No shared provider/journal/tools.
    for id in ids {
        let p = registry.get(&o.owner, id)?;
        if p.status != PalStatus::Active {
            return Err(AgentError::State("Pal is paused or archived"));
        }
        o.runtime(&p, ctx)?;
    }
    let mut tasks = vec![];
    for id in ids {
        let task = new_id();
        queue(registry, o, id, &task, prompt, None)?;
        tasks.push((id.clone(), task));
    }
    let mut results = vec![];
    // Scoped waves bound workers and join every thread, including panics and cancellation.
    for wave in tasks.chunks(concurrency) {
        std::thread::scope(|scope| {
            let jobs: Vec<_> = wave
                .iter()
                .map(|(id, task)| {
                    let per = per.clone();
                    scope.spawn(move || {
                        run_task(registry, o, id, task, ctx, per, &mut namzu_core::NoopEvents)
                    })
                })
                .collect();
            for ((id, task), job) in wave.iter().zip(jobs) {
                results.push(report(
                    id,
                    task,
                    job.join()
                        .unwrap_or(Err(AgentError::State("Pal worker panicked"))),
                ));
            }
        });
    }
    Ok(
        json!({"results":results,"allocation_per_pal":{"steps":per.max_steps,"tool_calls":per.max_tool_calls,"text_bytes":per.max_text_bytes},"concurrency":concurrency}),
    )
}
fn start_if_stopped(
    computer: &mut namzu_computer::Computer<'_>,
    ctx: &TurnContext,
) -> Result<(), AgentError> {
    if computer.snapshot().phase == namzu_computer::Phase::Stopped {
        computer.start(ctx)?;
    }
    Ok(())
}
fn computer(
    o: &PalOptions,
    registry: &Registry,
    id: &str,
    action: ComputerAction,
    ctx: &TurnContext,
) -> Result<Value, AgentError> {
    let manager = o.manager(registry, id)?;
    let mut prepared_browser = if let ComputerAction::Browser { request } = &action {
        let request: namzu_computer::browser::Request =
            serde_json::from_slice(&transfer::read(request)?)
                .map_err(|_| AgentError::Configuration("invalid browser JSON request"))?;
        request.validate()?;
        use namzu_computer::browser::Permission;
        let allowed = match request.permission() {
            Permission::Inspect => true,
            Permission::Navigate => o.allow_navigate,
            Permission::Act => o.allow_browser_actions,
            Permission::Upload => o.allow_upload,
            Permission::Download => o.allow_download,
        };
        if !allowed {
            return Err(AgentError::Configuration(
                "explicit permission required: --allow-navigate / --allow-browser-actions / --allow-upload / --allow-download",
            ));
        }
        if !manager.snapshot()?.is_some_and(|r| r.spec.browser) {
            return Err(AgentError::Configuration("browser not configured"));
        }
        Some(request)
    } else {
        None
    };
    let mut prepared_desktop = if let ComputerAction::Desktop { request, output } = &action {
        use namzu_computer::desktop::Request;
        let request: Request = serde_json::from_slice(&transfer::read(request)?)
            .map_err(|_| AgentError::Configuration("invalid desktop JSON request"))?;
        if !request.readonly() && !o.allow_input {
            return Err(AgentError::Configuration(
                "--allow-input required for guest input",
            ));
        }
        if matches!(request, Request::Screenshot) != output.is_some() {
            return Err(AgentError::Configuration(
                "only screenshot requires --output NEW_FILE",
            ));
        }
        let record = manager
            .snapshot()?
            .ok_or(AgentError::State("owned computer required"))?;
        request.validate(
            record
                .spec
                .desktop
                .as_ref()
                .ok_or(AgentError::Configuration("desktop not configured"))?,
        )?;
        Some(request)
    } else {
        None
    };
    match action {
        ComputerAction::Show => return Ok(json!(manager.snapshot()?)),
        ComputerAction::Cleanup => {
            manager.recover_cleanup(ctx)?;
            return Ok(json!({"cleaned":true}));
        }
        ComputerAction::Create {
            desktop,
            browser,
            width,
            height,
        } => {
            let pal = registry.get(&o.owner, id)?;
            let p: Profile = serde_json::from_str(&pal.route)
                .map_err(|_| AgentError::Configuration("CLI profile required"))?;
            if !p.guest || pal.status != PalStatus::Active {
                return Err(AgentError::Configuration("active guest Pal required"));
            }
            let image = o.image.as_ref().ok_or(AgentError::Configuration(
                "--image sha256:ID required for computer create",
            ))?;
            let mut spec = ComputerSpec::new(image);
            if browser {
                spec.browser = true;
                spec.memory_bytes = 768 * 1024 * 1024;
                spec.pids = 128;
            }
            if desktop {
                spec.desktop = Some(namzu_computer::desktop::ScreenConfig {
                    width: width.unwrap_or(1024),
                    height: height.unwrap_or(768),
                });
            }
            return Ok(json!(manager.acquire(spec, ctx)?.detach(ctx)?));
        }
        _ => (),
    }
    let keep_running = (manager.snapshot()?.is_some_and(|r| r.spec.browser)
        && matches!(
            action,
            ComputerAction::Upload { .. } | ComputerAction::Download { .. }
        ))
        || matches!(
            action,
            ComputerAction::Start
                | ComputerAction::Restart
                | ComputerAction::Desktop { .. }
                | ComputerAction::Browser { .. }
        );
    let mut lease = manager.resume(ctx)?;
    lease.preserve_workspace_on_drop();
    let result = (|| -> Result<Value, AgentError> {
        Ok(match action {
            ComputerAction::Browser { .. } => {
                let request = prepared_browser
                    .take()
                    .ok_or(AgentError::Configuration("browser request missing"))?;
                start_if_stopped(&mut lease, ctx)?;
                json!(lease.browser(request, ctx)?)
            }
            ComputerAction::Desktop { request: _, output } => {
                use base64::Engine as _;
                use namzu_computer::desktop::Reply;
                let request = prepared_desktop
                    .take()
                    .ok_or(AgentError::Configuration("desktop request missing"))?;
                start_if_stopped(&mut lease, ctx)?;
                match lease.desktop(request, ctx)? {
                    Reply::Screenshot {
                        width,
                        height,
                        png_base64,
                    } => {
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(png_base64)
                            .map_err(|_| AgentError::Protocol("invalid screenshot"))?;
                        transfer::write_new(
                            &output.ok_or(AgentError::Configuration("output required"))?,
                            &bytes,
                        )?;
                        json!({"width":width,"height":height,"bytes":bytes.len()})
                    }
                    value => json!(value),
                }
            }
            ComputerAction::Start => {
                start_if_stopped(&mut lease, ctx)?;
                json!({"started":true})
            }
            ComputerAction::Stop => {
                lease.stop(ctx)?;
                json!({"stopped":true})
            }
            ComputerAction::Restart => {
                lease.restart(ctx)?;
                json!({"restarted":true})
            }
            ComputerAction::Acknowledge => {
                lease.acknowledge_uncertain()?;
                json!({"acknowledged":true})
            }
            ComputerAction::Upload { path, file } => {
                if !o.run.allow_write {
                    return Err(AgentError::Configuration(
                        "--allow-write required for explicit guest upload",
                    ));
                }
                let bytes = transfer::read(&file)?;
                start_if_stopped(&mut lease, ctx)?;
                lease.upload(&path, &bytes, ctx)?;
                json!({"bytes":bytes.len()})
            }
            ComputerAction::Download { path, file } => {
                start_if_stopped(&mut lease, ctx)?;
                let bytes = lease.download(&path, ctx)?;
                transfer::write_new(&file, &bytes)?;
                json!({"bytes":bytes.len()})
            }
            ComputerAction::List { path } => {
                start_if_stopped(&mut lease, ctx)?;
                json!(lease.list(&path, ctx)?)
            }
            _ => unreachable!(),
        })
    })();
    let cleanup = TurnContext::new(Duration::from_secs(20), Cancellation::default())?;
    if result.is_ok() && keep_running {
        lease.release(&cleanup)?;
    } else {
        lease.detach(&cleanup)?;
    }
    result
}
