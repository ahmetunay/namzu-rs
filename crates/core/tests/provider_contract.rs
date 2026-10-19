use namzu_core::{provider::*, *};
use serde_json::json;
fn ctx() -> TurnContext {
    TurnContext::new(std::time::Duration::from_secs(30), Cancellation::default()).unwrap()
}
#[test]
fn old_stats_and_optional_usage_roundtrip() {
    let stats: Stats = serde_json::from_value(json!({"provider_calls":1,"tool_calls":0})).unwrap();
    assert!(stats.usage.is_empty());
    assert!(serde_json::to_value(stats).unwrap().get("usage").is_none());
    let usage = TokenUsage {
        input_tokens: Some(0),
        ..Default::default()
    };
    usage.validate().unwrap();
    let mut total = UsageTotals::default();
    total
        .record(&ResponseMetadata {
            usage: Some(usage),
            cost: None,
        })
        .unwrap();
    assert_eq!(
        total.input_tokens.unwrap(),
        ReportedTotal { sum: 0, samples: 1 }
    );
    assert!(total.output_tokens.is_none());
}
#[test]
fn partial_usage_preserves_coverage_and_unknown_cost() {
    let mut totals = UsageTotals::default();
    for u in [
        Some(TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            ..Default::default()
        }),
        None,
    ] {
        totals
            .record(&ResponseMetadata {
                usage: u,
                cost: None,
            })
            .unwrap();
    }
    assert_eq!(totals.responses, 2);
    assert_eq!(totals.input_tokens.as_ref().unwrap().samples, 1);
    assert_eq!(totals.input_tokens.as_ref().unwrap().sum, 10);
    assert!(totals.reported_cost_nano_usd.is_none());
    assert!(totals.estimated_cost_nano_usd.is_none());
}
#[test]
fn bad_usage_is_rejected_and_totals_update_atomically() {
    for usage in [
        TokenUsage {
            input_tokens: Some(3),
            cached_input_tokens: Some(4),
            ..Default::default()
        },
        TokenUsage {
            output_tokens: Some(2),
            reasoning_tokens: Some(3),
            ..Default::default()
        },
        TokenUsage {
            input_tokens: Some(3),
            output_tokens: Some(4),
            total_tokens: Some(8),
            ..Default::default()
        },
        TokenUsage {
            input_tokens: Some(u64::MAX),
            output_tokens: Some(1),
            total_tokens: Some(0),
            ..Default::default()
        },
    ] {
        assert!(usage.validate().is_err());
    }
    let mut t = UsageTotals::default();
    let m = ResponseMetadata {
        usage: Some(TokenUsage {
            input_tokens: Some(u64::MAX),
            ..Default::default()
        }),
        cost: None,
    };
    t.record(&m).unwrap();
    let before = t.clone();
    assert!(t.record(&m).is_err());
    assert_eq!(before, t);
}
#[test]
fn explicit_price_estimate_has_no_float_rounding_or_cache_double_count() {
    let p = Pricing {
        input_nano_usd_per_token: 2000,
        output_nano_usd_per_token: 8000,
        cached_input_nano_usd_per_token: Some(500),
    };
    let usage = TokenUsage {
        input_tokens: Some(100),
        output_tokens: Some(10),
        cached_input_tokens: Some(40),
        reasoning_tokens: Some(5),
        ..Default::default()
    };
    assert_eq!(
        p.estimate(&usage).unwrap(),
        Some(Cost {
            nano_usd: 220000,
            source: CostSource::Estimated
        })
    );
    assert!(
        p.estimate(&TokenUsage {
            input_tokens: Some(100),
            output_tokens: Some(10),
            ..Default::default()
        })
        .unwrap()
        .is_none()
    );
    assert!(p.estimate(&TokenUsage::default()).unwrap().is_none());
    let p = Pricing {
        input_nano_usd_per_token: u64::MAX,
        output_nano_usd_per_token: 0,
        cached_input_nano_usd_per_token: None,
    };
    assert!(
        p.estimate(&TokenUsage {
            input_tokens: Some(2),
            output_tokens: Some(0),
            ..Default::default()
        })
        .is_err()
    );
}
#[test]
fn estimates_and_reported_costs_never_merge_as_one_bill() {
    let mut t = UsageTotals::default();
    for source in [CostSource::Reported, CostSource::Estimated] {
        t.record(&ResponseMetadata {
            usage: None,
            cost: Some(Cost {
                nano_usd: 100,
                source,
            }),
        })
        .unwrap();
    }
    assert_eq!(t.reported_cost_nano_usd.unwrap().sum, 100);
    assert_eq!(t.estimated_cost_nano_usd.unwrap().sum, 100);
}
#[test]
fn typed_errors_have_no_server_payload_and_bounded_retry_hint() {
    for (status, kind) in [
        (401, FailureKind::Authentication),
        (403, FailureKind::Permission),
        (404, FailureKind::NotFound),
        (429, FailureKind::RateLimited),
        (503, FailureKind::Unavailable),
        (400, FailureKind::InvalidRequest),
    ] {
        let f = ProviderFailure::http(status, Some(10));
        assert_eq!(f.kind, kind);
        assert_eq!(f.retry_after_seconds, Some(10));
        assert!(f.to_string().contains(&status.to_string()));
    }
    assert_eq!(
        ProviderFailure::http(429, Some(86401)).retry_after_seconds,
        None
    );
    assert!(serde_json::from_value::<ProviderFailure>(json!({"kind":"transport","http_status":null,"retry_after_seconds":null,"body":"private"})).is_err());
}
#[test]
fn common_request_bounds_duplicate_schemas_and_declared_unsupported_features() {
    let mut info = ProviderInfo::default();
    let tools = [ToolDefinition {
        name: "read".into(),
        description: "fixture".into(),
        parameters: json!({"type":"object"}),
    }];
    validate_request(
        &[Message::User {
            content: "fixture".into(),
        }],
        &tools,
        &info,
        &ctx(),
    )
    .unwrap();
    info.capabilities.tools = Support::Unsupported;
    assert!(validate_request(&[], &tools, &info, &ctx()).is_err());
    assert!(
        validate_request(
            &[],
            &[tools[0].clone(), tools[0].clone()],
            &ProviderInfo::default(),
            &ctx()
        )
        .is_err()
    );
    assert!(
        validate_request(
            &[Message::User {
                content: "x".repeat(4 * 1024 * 1024)
            }],
            &[],
            &ProviderInfo::default(),
            &ctx()
        )
        .is_err()
    );
    let c = ctx();
    c.cancellation().cancel();
    assert_eq!(
        validate_request(&[], &[], &ProviderInfo::default(), &c),
        Err(AgentError::Cancelled)
    );
}
#[test]
fn common_completion_validation_rejects_bad_calls_before_effects() {
    let c = ToolCall {
        id: "x".into(),
        name: "write".into(),
        arguments: json!({}),
    };
    assert!(
        validate_completion(&Completion {
            text: String::new(),
            tool_calls: vec![c.clone(), c]
        })
        .is_err()
    );
    for (name, args) in [("bad\nname", json!({})), ("read", json!([]))] {
        assert!(
            validate_completion(&Completion {
                text: String::new(),
                tool_calls: vec![ToolCall {
                    id: "x".into(),
                    name: name.into(),
                    arguments: args
                }]
            })
            .is_err()
        );
    }
}
struct Metered {
    next: usize,
}
impl Provider for Metered {
    fn complete(
        &mut self,
        _: &[Message],
        _: &[ToolDefinition],
        _: &TurnContext,
    ) -> Result<Completion, AgentError> {
        self.next += 1;
        Ok(Completion {
            text: "done".into(),
            tool_calls: vec![],
        })
    }
    fn take_metadata(&mut self) -> Option<ResponseMetadata> {
        Some(ResponseMetadata {
            usage: Some(TokenUsage {
                input_tokens: Some(12),
                output_tokens: Some(2),
                total_tokens: Some(14),
                ..Default::default()
            }),
            cost: None,
        })
    }
}
struct NoTools;
impl Tools for NoTools {
    fn definitions(&self) -> Vec<ToolDefinition> {
        vec![]
    }
    fn call(&mut self, _: &ToolCall, _: &TurnContext) -> Result<ToolResult, AgentError> {
        panic!("no tool effect expected")
    }
}
#[derive(Default)]
struct Journal(Vec<Event>);
impl Observer for Journal {
    fn checkpoint(
        &mut self,
        _: &[Message],
        s: &Stats,
        _: Option<&ToolCall>,
    ) -> Result<(), AgentError> {
        if s.provider_calls > 0 {
            assert_eq!(s.usage.total_tokens.as_ref().unwrap().sum, 14);
        }
        Ok(())
    }
    fn event(&mut self, e: Event) -> Result<(), AgentError> {
        self.0.push(e);
        Ok(())
    }
}
#[test]
fn successful_metadata_is_checkpointed_and_emitted_before_final_message() {
    let mut journal = Journal::default();
    let turn = run_history(
        &mut Metered { next: 0 },
        &mut NoTools,
        vec![Message::User {
            content: "fixture".into(),
        }],
        Limits::default(),
        Cancellation::default(),
        &mut journal,
    )
    .unwrap();
    assert_eq!(turn.stats.usage.total_tokens.unwrap().sum, 14);
    let index = journal
        .0
        .iter()
        .position(|e| matches!(e, Event::ProviderFinished { .. }))
        .unwrap();
    assert!(
        journal.0[index + 1..]
            .iter()
            .any(|e| matches!(e, Event::AssistantMessage { .. }))
    );
}
