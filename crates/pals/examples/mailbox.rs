//! Durable automatic message handling with real file tools; no model/account/network.
use namzu_core::{
    AgentError, Cancellation, Completion, Limits, NoopObserver, ScriptedProvider, ToolCall,
    TurnContext,
};
use namzu_pals::{
    BatchBudget, DeliveryState, HostRuntime, MailMessage, MailSender, MailboxHost, MailboxRuntime,
    PalCreate, PalDefinition, Registry, TaskStatus,
};
use namzu_tools::{TerminalTools, ToolPolicy};
use serde_json::json;

#[derive(Default)]
struct Host {
    opened: usize,
}
impl MailboxHost for Host {
    type Provider = ScriptedProvider;
    type Tools = TerminalTools;
    type Observer = NoopObserver;
    fn open(
        &mut self,
        pal: &PalDefinition,
        ctx: &TurnContext,
    ) -> Result<HostRuntime<Self>, AgentError> {
        ctx.remaining()?;
        self.opened += 1;
        let tools = TerminalTools::new(&pal.workspace, ToolPolicy::default())
            .map_err(|_| AgentError::State("cannot open demo workspace"))?;
        Ok(MailboxRuntime {
            provider: ScriptedProvider(
                vec![
                    Completion {
                        text: String::new(),
                        tool_calls: vec![ToolCall {
                            id: "read".into(),
                            name: "read_file".into(),
                            arguments: json!({"path":"answer.txt"}),
                        }],
                    },
                    Completion {
                        text: "12".into(),
                        tool_calls: vec![],
                    },
                ]
                .into(),
            ),
            tools,
            observer: NoopObserver,
        })
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = tempfile::tempdir()?;
    let state = root.path().join("state");
    let workspaces = root.path().join("workspaces");
    let registry = Registry::open(&state, &workspaces)?;
    let create = |name: &str| PalCreate {
        owner: "demo".into(),
        name: name.into(),
        purpose: "Read a fixture".into(),
        route: "scripted-fixture".into(),
        tools: None,
    };
    let sender = registry.create(create("Sender"))?;
    let receiver = registry.create(create("Receiver"))?;
    std::fs::write(receiver.workspace.join("answer.txt"), "12\n")?;
    let mailbox = registry.mailbox("demo", &receiver.id)?;
    let message = MailMessage::operator("operator-request", "Read answer.txt");
    assert!(mailbox.send(message.clone())?.inserted);
    assert!(!mailbox.send(message.clone())?.inserted);
    mailbox.send(MailMessage {
        id: "pal-request".into(),
        sender: MailSender::Pal { id: sender.id },
        body: "Read answer.txt".into(),
        in_reply_to: Some("operator-request".into()),
    })?;
    let ctx = TurnContext::new(Limits::default().timeout, Cancellation::default())?;
    let mut host = Host::default();
    let batch = mailbox.drain("demo-worker", &ctx, BatchBudget::default(), &mut host)?;
    assert!(batch.stopped.is_none());
    assert_eq!(batch.deliveries.len(), 2);
    for delivery in batch.deliveries {
        let turn = delivery.result?;
        assert_eq!(turn.output, "12");
        assert_eq!(turn.stats.tool_calls, 1);
    }
    assert_eq!(host.opened, 2);
    assert!(!mailbox.send(message)?.inserted);
    drop(mailbox);
    drop(registry);
    let restored = Registry::open(&state, &workspaces)?;
    let mailbox = restored.mailbox("demo", &receiver.id)?;
    assert!(mailbox.messages()?.iter().all(|e| matches!(
        e.delivery,
        DeliveryState::Acknowledged {
            outcome: TaskStatus::Complete,
            ..
        }
    )));
    assert!(
        mailbox
            .process_one("new-worker", &ctx, Limits::default(), &mut host)?
            .is_none()
    );
    assert_eq!(host.opened, 2);
    println!(
        "{}",
        json!({"offline":true,"processed_messages":2,"real_file_reads":2,
        "duplicate_suppression":"passed","reopened_acknowledgements":"passed"})
    );
    Ok(())
}
