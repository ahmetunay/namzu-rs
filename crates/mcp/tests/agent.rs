#![cfg(unix)]
use namzu_core::{
    AgentError, Cancellation, Completion, Limits, Message, NoopObserver, ScriptedProvider,
    ToolCall, ToolDefinition, ToolResult, Tools, TurnContext, run_history_in_context,
};
use namzu_mcp::{
    agent::{AgentTools, Approvals, ApprovedObserver, alias},
    config::{Document, Policy, Registry},
    discovery::Kind,
    manager::Manager,
    schema::Compiled,
};
use serde_json::{Value, json};
use std::{cell::Cell, path::Path, rc::Rc, time::Duration};
struct Local(Rc<Cell<usize>>);
impl Tools for Local {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            name: "local_effect".into(),
            description: "local fixture".into(),
            parameters: json!({"type":"object"}),
        }]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        self.0.set(self.0.get() + 1);
        Ok(ToolResult::ok("local"))
    }
}
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(10), Cancellation::default()).unwrap()
}
fn registry(root: &Path, mode: &str, ask: bool) -> Registry {
    let r = Registry::open(&root.join("state")).unwrap();
    let doc = json!({"version":1,"default_profile":"default","profiles":{"default":{"servers":{"fixture":{"transport":{"type":"stdio","program":"/usr/bin/python3","args":[format!("{}/tests/fixtures/discovery_server.py",env!("CARGO_MANIFEST_DIR")),mode],"cwd":root},"policy":{"require_approval":ask}}}}}});
    r.replace(Document::parse(doc.to_string().as_bytes()).unwrap())
        .unwrap();
    r
}
fn call(id: &str, name: &str, args: Value) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: name.into(),
        arguments: args,
    }
}
fn tool_calls(root: &Path) -> usize {
    std::fs::read_to_string(root.join("requests.jsonl"))
        .unwrap()
        .lines()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == "tools/call")
        .count()
}
#[test]
fn scripted_core_executes_mcp_and_preserves_structured_content() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let effects = Rc::new(Cell::new(0));
    let mut tools = AgentTools::new(Local(effects), m, Approvals::default());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })
        .unwrap();
    let name = alias("fixture", Kind::Tools, "read");
    assert!(name.len() <= 64);
    let mut provider = ScriptedProvider(
        [
            Completion {
                text: "".into(),
                tool_calls: vec![call("one", &name, json!({"path":"a"}))],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let result = run_history_in_context(
        &mut provider,
        &mut tools,
        vec![Message::User {
            content: "test".into(),
        }],
        Limits::default(),
        &ctx,
        &mut NoopObserver,
    )
    .unwrap();
    assert_eq!(result.stats.tool_calls, 1);
    assert!(result.messages.iter().any(|m|matches!(m,Message::Tool{content,is_error:false,..} if serde_json::from_str::<Value>(content).unwrap()["structuredContent"]["answer"]==42)));
    tools.close(&ctx).unwrap();
    assert_eq!(tool_calls(root.path()), 1);
}
#[test]
fn entire_batch_is_schema_checked_before_any_local_or_remote_effect() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let effects = Rc::new(Cell::new(0));
    let mut tools = AgentTools::new(Local(effects.clone()), m, Approvals::default());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })
        .unwrap();
    let mut provider = ScriptedProvider(
        [Completion {
            text: "".into(),
            tool_calls: vec![
                call("local", "local_effect", json!({})),
                call(
                    "bad",
                    &alias("fixture", Kind::Tools, "read"),
                    json!({"path":4}),
                ),
            ],
        }]
        .into(),
    );
    assert!(
        run_history_in_context(
            &mut provider,
            &mut tools,
            vec![Message::User {
                content: "test".into()
            }],
            Limits::default(),
            &ctx,
            &mut NoopObserver
        )
        .is_err()
    );
    assert_eq!(effects.get(), 0);
    assert_eq!(tool_calls(root.path()), 0);
    tools.close(&ctx).unwrap();
}
#[test]
fn output_schema_failure_is_fatal_without_replay() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "agent_bad_output", false);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), m, Approvals::default());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })
        .unwrap();
    assert!(
        tools
            .call(
                &call(
                    "one",
                    &alias("fixture", Kind::Tools, "read"),
                    json!({"path":"a"})
                ),
                &ctx
            )
            .is_err()
    );
    tools.close(&ctx).unwrap();
    assert_eq!(tool_calls(root.path()), 1);
}
#[test]
fn approvals_are_bound_single_use_and_not_implicit_noop_grants() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", true);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let grants = Approvals::default();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), m, grants.clone());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.approve()
        })
        .unwrap();
    let c = call(
        "one",
        &alias("fixture", Kind::Tools, "read"),
        json!({"path":"a"}),
    );
    assert!(tools.call(&c, &ctx).unwrap().is_error);
    grants.grant(&c).unwrap();
    let mut altered = c.clone();
    altered.arguments = json!({"path":"other"});
    assert!(tools.call(&altered, &ctx).unwrap().is_error);
    assert!(!tools.call(&c, &ctx).unwrap().is_error);
    assert!(tools.call(&c, &ctx).unwrap().is_error);
    tools.close(&ctx).unwrap();
    assert_eq!(tool_calls(root.path()), 1);
}
#[test]
fn compiled_schemas_enforce_refs_combinators_patterns_and_closed_objects() {
    let schema = json!({"type":"object","$defs":{"positive":{"type":"integer","minimum":1}},"properties":{"n":{"$ref":"#/$defs/positive"},"s":{"type":"string","pattern":"^[a-z]{2,4}$"}},"required":["n","s"],"additionalProperties":false,"oneOf":[{"properties":{"n":{"maximum":3}}},{"properties":{"n":{"minimum":10}}}]});
    let compiled = Compiled::new(schema.as_object().unwrap()).unwrap();
    assert!(compiled.validate(&json!({"n":2,"s":"ok"}), &ctx()).is_ok());
    for value in [
        json!({"n":5,"s":"ok"}),
        json!({"n":0,"s":"ok"}),
        json!({"n":2,"s":"UPPER"}),
        json!({"n":2,"s":"ok","extra":true}),
    ] {
        assert!(compiled.validate(&value, &ctx()).is_err());
    }
}
#[test]
fn nonproductive_reference_cycles_and_external_retrieval_are_rejected() {
    for schema in [
        json!({"type":"object","$ref":"#"}),
        json!({"type":"object","$defs":{"A":{"$ref":"#/$defs/B"},"B":{"$ref":"#/$defs/A"}},"$ref":"#/$defs/A"}),
        json!({"type":"object","$ref":"file:///DO_NOT_READ"}),
        json!({"type":"object","properties":{"x":{"pattern":"("}}}),
    ] {
        assert!(Compiled::new(schema.as_object().unwrap()).is_err());
    }
    let recursive =
        json!({"type":"object","properties":{"child":{"$ref":"#"}},"additionalProperties":false});
    let validator = Compiled::new(recursive.as_object().unwrap()).unwrap();
    assert!(
        validator
            .validate(&json!({"child":{"child":{}}}), &ctx())
            .is_ok()
    );
}
#[test]
fn rpc_and_tool_failures_stay_model_data_and_do_not_leak_remote_error_text() {
    for mode in ["agent_rpc_error", "agent_tool_error"] {
        let root = tempfile::tempdir().unwrap();
        let r = registry(root.path(), mode, false);
        let ctx = ctx();
        let m = Manager::new(
            &r,
            None,
            None,
            Policy {
                require_approval: false,
                ..Policy::default()
            },
            &ctx,
        )
        .unwrap();
        let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), m, Approvals::default());
        tools
            .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
                p.permit()
            })
            .unwrap();
        let result = tools
            .call(
                &call(
                    "one",
                    &alias("fixture", Kind::Tools, "read"),
                    json!({"path":"a"}),
                ),
                &ctx,
            )
            .unwrap();
        assert!(result.is_error);
        assert!(!result.content.contains("DO_NOT_LOG_REMOTE_SECRET"));
        tools.close(&ctx).unwrap();
    }
}
#[test]
fn child_ceiling_and_alias_namespace_are_preserved() {
    assert_ne!(
        alias("one", Kind::Tools, "read"),
        alias("two", Kind::Tools, "read")
    );
    assert_ne!(
        alias("one", Kind::Tools, "read"),
        alias("one", Kind::Prompts, "read")
    );
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), m, Approvals::default());
    let allowed = [alias("fixture", Kind::Tools, "read")]
        .into_iter()
        .collect();
    tools
        .load(
            &["fixture".into()],
            Some(&allowed),
            &Default::default(),
            &ctx,
            |p| p.permit(),
        )
        .unwrap();
    assert_eq!(
        tools
            .definitions()
            .iter()
            .filter(|d| d.name.starts_with("mcp_"))
            .count(),
        1
    );
    tools.close(&ctx).unwrap();
}
#[test]
fn observer_approval_grants_only_the_exact_following_call() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", true);
    let ctx = ctx();
    let m = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let approvals = Approvals::default();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), m, approvals.clone());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.approve()
        })
        .unwrap();
    let mut provider = ScriptedProvider(
        [
            Completion {
                text: "".into(),
                tool_calls: vec![call(
                    "one",
                    &alias("fixture", Kind::Tools, "read"),
                    json!({"path":"a"}),
                )],
            },
            Completion {
                text: "done".into(),
                tool_calls: vec![],
            },
        ]
        .into(),
    );
    let mut inner = NoopObserver;
    let mut observer = ApprovedObserver {
        inner: &mut inner,
        approvals,
        explicit: true,
    };
    run_history_in_context(
        &mut provider,
        &mut tools,
        vec![Message::User {
            content: "test".into(),
        }],
        Limits::default(),
        &ctx,
        &mut observer,
    )
    .unwrap();
    tools.close(&ctx).unwrap();
    assert_eq!(tool_calls(root.path()), 1);
}
#[test]
fn resources_and_prompt_roles_remain_tool_data() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let manager = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), manager, Approvals::default());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })
        .unwrap();
    let resource = tools
        .call(
            &call(
                "r",
                &alias("fixture", Kind::Resources, "mem://resource/α"),
                json!({}),
            ),
            &ctx,
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&resource.content).unwrap()["contents"][0]["text"],
        "hello"
    );
    let prompt = tools
        .call(
            &call(
                "p",
                &alias("fixture", Kind::Prompts, "review"),
                json!({"path":"source.rs"}),
            ),
            &ctx,
        )
        .unwrap();
    let data: Value = serde_json::from_str(&prompt.content).unwrap();
    assert_eq!(data["messages"][0]["role"], "user");
    assert_eq!(data["messages"][0]["content"]["text"], "Review source.rs");
    assert!(
        tools
            .validate(
                &call(
                    "bad",
                    &alias("fixture", Kind::Prompts, "review"),
                    json!({"path":42})
                ),
                &ctx
            )
            .is_err()
    );
    tools.close(&ctx).unwrap();
}
#[test]
fn changed_configuration_cannot_reuse_a_loaded_binding() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let manager = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), manager, Approvals::default());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.permit()
        })
        .unwrap();
    registry(root.path(), "agent_bad_output", false);
    assert!(
        tools
            .call(
                &call(
                    "one",
                    &alias("fixture", Kind::Tools, "read"),
                    json!({"path":"a"})
                ),
                &ctx
            )
            .is_err()
    );
    tools.close(&ctx).unwrap();
    assert_eq!(tool_calls(root.path()), 0);
}
#[test]
fn guest_switch_retires_host_connection_and_old_permit() {
    struct RejectGuest;
    impl namzu_mcp::manager::GuestStdio for RejectGuest {
        fn launch(
            &mut self,
            _: namzu_mcp::stdio::Spec,
            _: &TurnContext,
        ) -> Result<namzu_mcp::stdio::Spec, namzu_mcp::config::Error> {
            Err(namzu_mcp::config::Error::Unsupported)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", false);
    let ctx = ctx();
    let mut manager = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let connect = manager
        .prepare_connect("fixture")
        .unwrap()
        .permit()
        .unwrap();
    manager.execute(connect, &ctx).unwrap();
    manager
        .discover("fixture", Kind::Tools, &ctx, |p| p.permit())
        .unwrap();
    let old = manager
        .prepare_call("fixture", "ping", None)
        .unwrap()
        .permit()
        .unwrap();
    let mut manager = manager.with_guest(Box::new(RejectGuest));
    assert!(manager.statuses().is_empty());
    assert!(manager.catalog("fixture", Kind::Tools).unwrap().is_none());
    assert!(matches!(
        manager.execute(old, &ctx),
        Err(namzu_mcp::config::Error::Stale)
    ));
    let connect = manager
        .prepare_connect("fixture")
        .unwrap()
        .permit()
        .unwrap();
    assert!(matches!(
        manager.execute(connect, &ctx),
        Err(namzu_mcp::config::Error::Unsupported)
    ));
    assert_eq!(tool_calls(root.path()), 0);
}
#[test]
fn explicit_rebind_requires_new_approvals_and_failed_rebind_has_no_old_tools() {
    let root = tempfile::tempdir().unwrap();
    let r = registry(root.path(), "normal", true);
    let ctx = ctx();
    let manager = Manager::new(
        &r,
        None,
        None,
        Policy {
            require_approval: false,
            ..Policy::default()
        },
        &ctx,
    )
    .unwrap();
    let approvals = Approvals::default();
    let mut tools = AgentTools::new(Local(Rc::new(Cell::new(0))), manager, approvals.clone());
    tools
        .load(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.approve()
        })
        .unwrap();
    let pending = call(
        "old",
        &alias("fixture", Kind::Tools, "read"),
        json!({"path":"a"}),
    );
    approvals.grant(&pending).unwrap();
    let p = tools
        .manager_mut()
        .prepare_reconnect("fixture")
        .unwrap()
        .approve()
        .unwrap();
    tools.manager_mut().execute(p, &ctx).unwrap();
    assert!(tools.validate(&pending, &ctx).is_err());
    tools
        .rebind(&["fixture".into()], None, &Default::default(), &ctx, |p| {
            p.approve()
        })
        .unwrap();
    assert!(tools.call(&pending, &ctx).unwrap().is_error);
    assert_eq!(tool_calls(root.path()), 0);
    approvals.grant(&pending).unwrap();
    assert!(!tools.call(&pending, &ctx).unwrap().is_error);
    assert_eq!(tool_calls(root.path()), 1);
    assert!(
        tools
            .rebind(&["fixture".into()], None, &Default::default(), &ctx, |_| {
                Err(namzu_mcp::config::Error::Denied)
            })
            .is_err()
    );
    assert!(
        !tools
            .definitions()
            .iter()
            .any(|d| d.name.starts_with("mcp_"))
    );
    tools.close(&ctx).unwrap();
}
