use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_computer::{
    Answer, ComputerManager, ComputerRecord, ComputerSpec, ContainerInfo, Engine, GuestPolicy,
    GuestTools, Mount, Operation, Phase, VolumeInfo, labels,
    protocol::{FILE_LIMIT, GuestReply, GuestRequest, GuestValue},
};
use namzu_core::{AgentError, Cancellation, ToolCall, Tools, TurnContext};
use namzu_session::PrivateStore;
use namzu_tools::process::ProcessOutput;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct State {
    container: Option<ContainerInfo>,
    volume: Option<VolumeInfo>,
    files: BTreeMap<String, Vec<u8>>,
    execs: usize,
    fail_create: bool,
    screen: Option<namzu_computer::desktop::ScreenConfig>,
    fail_desktop: bool,
    fail_browser: bool,
    wrong_browser: bool,
    fail_start: bool,
    cancel_exec: bool,
    bad_reply: bool,
    large_reply: bool,
    fail_volume_remove: bool,
    output_limit: bool,
    removes: usize,
    fail_kill: bool,
    cancel_volume: bool,
}
#[derive(Clone, Default)]
struct Mock(Arc<Mutex<State>>);
fn info(r: &ComputerRecord) -> ContainerInfo {
    ContainerInfo {
        id: "c".repeat(64),
        name: format!("/{}", r.container),
        image: r.spec.image.clone(),
        running: false,
        labels: labels(r),
        memory: r.spec.memory_bytes,
        swap: r.spec.memory_bytes as i64,
        cpus: u64::from(r.spec.cpu_millis) * 1_000_000,
        pids: i64::from(r.spec.pids),
        readonly: true,
        privileged: false,
        user: "65532:65532".into(),
        network: "none".into(),
        caps: vec!["ALL".into()],
        security: vec!["no-new-privileges:true".into()],
        mounts: vec![Mount {
            kind: "volume".into(),
            name: r.volume.clone(),
            destination: "/workspace".into(),
        }],
        binds: vec![],
        devices: vec![],
        restart: "no".into(),
    }
}
impl Engine for Mock {
    fn perform(&mut self, op: Operation, ctx: &TurnContext) -> Result<Answer, AgentError> {
        ctx.remaining()?;
        let mut s = self.0.lock().unwrap();
        match op {
            Operation::Image(_) => Ok(Answer::Image(true)),
            Operation::Inspect(_) => Ok(Answer::Container(s.container.clone().map(Box::new))),
            Operation::VolumeInspect(_) => Ok(Answer::Volume(s.volume.clone())),
            Operation::CreateVolume { record } => {
                s.volume = Some(VolumeInfo {
                    name: record.volume.clone(),
                    labels: labels(&record),
                });
                if s.cancel_volume {
                    ctx.cancellation().cancel();
                }
                Ok(Answer::Done)
            }
            Operation::Create { record } => {
                s.screen = record.spec.desktop.clone();
                s.container = Some(info(&record));
                if s.fail_create {
                    Err(AgentError::State("fixture ambiguous create"))
                } else {
                    Ok(Answer::Done)
                }
            }
            Operation::Start(_) => {
                s.container.as_mut().unwrap().running = true;
                if s.fail_start {
                    Err(AgentError::State("fixture ambiguous start"))
                } else {
                    Ok(Answer::Done)
                }
            }
            Operation::Kill(_) => {
                if s.fail_kill {
                    return Err(AgentError::State("fixture kill failed"));
                }
                s.container.as_mut().unwrap().running = false;
                Ok(Answer::Done)
            }
            Operation::Remove(_) => {
                s.container = None;
                s.removes += 1;
                Ok(Answer::Done)
            }
            Operation::RemoveVolume(_) => {
                if s.fail_volume_remove {
                    return Err(AgentError::State("fixture volume busy"));
                }
                s.volume = None;
                s.files.clear();
                Ok(Answer::Done)
            }
            Operation::Exec { request, .. } => {
                s.execs += 1;
                if s.cancel_exec {
                    s.cancel_exec = false;
                    ctx.cancellation().cancel();
                    return Err(AgentError::Cancelled);
                }
                if s.bad_reply {
                    s.bad_reply = false;
                    return Ok(Answer::Guest(GuestReply {
                        version: 99,
                        result: Ok(GuestValue::Ready),
                    }));
                }
                let value = match request {
                    GuestRequest::Browser { request, .. } => {
                        use namzu_computer::browser::{Page, Reply, Request};
                        if s.fail_browser {
                            return Ok(Answer::Guest(GuestReply {
                                version: 1,
                                result: Err("fixture browser failure".into()),
                            }));
                        }
                        let page = Page {
                            url: "about:blank".into(),
                            title: "fixture".into(),
                            frame_id: "frame".into(),
                            loader_id: "loader".into(),
                        };
                        let value = if s.wrong_browser {
                            Reply::Info { page }
                        } else {
                            match request {
                                Request::Info => Reply::Info { page },
                                Request::Navigate { .. } => Reply::Navigated { page },
                                Request::Snapshot => Reply::Snapshot {
                                    page,
                                    dom: vec![],
                                    accessibility: vec![],
                                    truncated: false,
                                },
                            }
                        };
                        GuestValue::Browser { value }
                    }
                    GuestRequest::Desktop { request, .. } => {
                        if s.fail_desktop {
                            return Ok(Answer::Guest(GuestReply {
                                version: 1,
                                result: Err("fixture desktop failure".into()),
                            }));
                        }
                        use namzu_computer::desktop::{Reply, Request};
                        let screen = s.screen.as_ref().unwrap();
                        let value = match request {
                            Request::Info => Reply::Info {
                                width: screen.width,
                                height: screen.height,
                                pointer_x: 0,
                                pointer_y: 0,
                            },
                            Request::Screenshot => Reply::Screenshot {
                                width: screen.width,
                                height: screen.height,
                                png_base64: STANDARD.encode(blank_png(screen.width, screen.height)),
                            },
                            _ => Reply::Applied,
                        };
                        GuestValue::Desktop { value }
                    }
                    GuestRequest::Probe => GuestValue::Ready,
                    GuestRequest::Write { path, base64 } => {
                        s.files.insert(path, STANDARD.decode(base64).unwrap());
                        GuestValue::Written
                    }
                    GuestRequest::Read { path } => GuestValue::Bytes {
                        base64: STANDARD.encode(if s.large_reply {
                            vec![0; FILE_LIMIT + 1]
                        } else {
                            s.files.get(&path).cloned().unwrap_or_default()
                        }),
                    },
                    GuestRequest::List { .. } => GuestValue::Entries {
                        names: s.files.keys().cloned().collect(),
                    },
                    GuestRequest::Shell { .. } => GuestValue::Shell {
                        output: ProcessOutput {
                            status: if s.output_limit {
                                "output_limit".into()
                            } else {
                                "exited".into()
                            },
                            exit_code: Some(0),
                            stdout: "guest".into(),
                            stderr: String::new(),
                            peak_rss_kib: None,
                        },
                    },
                };
                Ok(Answer::Guest(GuestReply {
                    version: 1,
                    result: Ok(value),
                }))
            }
        }
    }
}
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(30), Cancellation::default()).unwrap()
}
fn spec() -> ComputerSpec {
    ComputerSpec::new(format!("sha256:{}", "a".repeat(64)))
}
fn setup() -> (tempfile::TempDir, Mock, ComputerManager<Mock>) {
    let dir = tempfile::tempdir().unwrap();
    let mock = Mock::default();
    let manager = ComputerManager::from_store(
        PrivateStore::open(dir.path()).unwrap(),
        "owner",
        "pal",
        mock.clone(),
    )
    .unwrap();
    (dir, mock, manager)
}
#[test]
fn lifecycle_binary_transfer_restart_and_drop_cleanup() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    computer.upload("binary", &[0, 255, 10], &ctx()).unwrap();
    assert_eq!(computer.download("binary", &ctx()).unwrap(), [0, 255, 10]);
    computer.restart(&ctx()).unwrap();
    assert_eq!(computer.download("binary", &ctx()).unwrap(), [0, 255, 10]);
    drop(computer);
    assert_eq!(manager.snapshot().unwrap().unwrap().phase, Phase::Closed);
    assert!(mock.0.lock().unwrap().container.is_none());
    assert!(mock.0.lock().unwrap().volume.is_none());
}
#[test]
fn ambiguous_create_and_start_are_cleaned_without_touching_other_resources() {
    for point in ["create", "start"] {
        let (_root, mock, manager) = setup();
        {
            let mut s = mock.0.lock().unwrap();
            s.fail_create = point == "create";
            s.fail_start = point == "start";
        }
        assert!(manager.acquire(spec(), &ctx()).is_err());
        let s = mock.0.lock().unwrap();
        assert!(s.container.is_none());
        assert!(s.volume.is_none());
        assert_eq!(manager.snapshot().unwrap().unwrap().phase, Phase::Closed);
    }
}
#[test]
fn owner_labels_on_both_resources_are_checked_before_any_removal() {
    for resource in ["container", "volume"] {
        let (_root, mock, manager) = setup();
        let mut computer = manager.acquire(spec(), &ctx()).unwrap();
        {
            let mut s = mock.0.lock().unwrap();
            let labels = if resource == "container" {
                &mut s.container.as_mut().unwrap().labels
            } else {
                &mut s.volume.as_mut().unwrap().labels
            };
            labels.insert("io.namzu.computer.owner".into(), "foreign".into());
        }
        assert!(computer.close(&ctx()).is_err());
        drop(computer);
        let s = mock.0.lock().unwrap();
        assert_eq!(s.removes, 0);
        assert!(s.container.is_some());
        assert!(s.volume.is_some());
    }
}
#[test]
fn cancellation_stops_remote_computer_preserves_uncertainty_and_never_replays() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    computer.upload("known", b"kept", &ctx()).unwrap();
    mock.0.lock().unwrap().cancel_exec = true;
    assert_eq!(
        computer.upload("uncertain", b"data", &ctx()).unwrap_err(),
        AgentError::Cancelled
    );
    assert_eq!(computer.snapshot().phase, Phase::Stopped);
    assert!(computer.snapshot().pending.is_some());
    assert!(!mock.0.lock().unwrap().container.as_ref().unwrap().running);
    drop(computer);
    let before = mock.0.lock().unwrap().execs;
    let mut computer = manager.resume(&ctx()).unwrap();
    assert_eq!(mock.0.lock().unwrap().execs, before);
    computer.start(&ctx()).unwrap();
    assert_eq!(computer.download("known", &ctx()).unwrap(), b"kept");
    assert!(computer.snapshot().pending.is_some());
    assert!(computer.upload("new", b"no", &ctx()).is_err());
    computer.acknowledge_uncertain().unwrap();
    computer.upload("new", b"yes", &ctx()).unwrap();
    computer.close(&ctx()).unwrap();
}
#[test]
fn malformed_and_oversized_guest_replies_stop_and_keep_pending() {
    for bad in [true, false] {
        let (_root, mock, manager) = setup();
        let mut computer = manager.acquire(spec(), &ctx()).unwrap();
        {
            let mut s = mock.0.lock().unwrap();
            s.bad_reply = bad;
            s.large_reply = !bad;
        }
        assert!(computer.download("file", &ctx()).is_err());
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert!(computer.snapshot().pending.is_some());
        computer.close(&ctx()).unwrap();
    }
}
#[test]
fn output_limit_stops_all_guest_work_but_does_not_repeat_command() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    mock.0.lock().unwrap().output_limit = true;
    let out = computer
        .shell("flood", ".", Duration::from_secs(1), &ctx())
        .unwrap();
    assert_eq!(out.status, "output_limit");
    assert_eq!(computer.snapshot().phase, Phase::Stopped);
    assert!(
        computer
            .shell("new", ".", Duration::from_secs(1), &ctx())
            .is_err()
    );
    computer.close(&ctx()).unwrap();
}
#[test]
fn detached_computer_resumes_exclusively_and_new_creation_increments_generation() {
    let (root, mock, manager) = setup();
    let mut c = manager.acquire(spec(), &ctx()).unwrap();
    c.upload("data", b"kept", &ctx()).unwrap();
    let initial = c.snapshot().clone();
    assert!(manager.resume(&ctx()).is_err());
    c.detach(&ctx()).unwrap();
    assert!(manager.acquire(spec(), &ctx()).is_err());
    let other = ComputerManager::from_store(
        PrivateStore::open(root.path()).unwrap(),
        "owner",
        "pal",
        mock.clone(),
    )
    .unwrap();
    let mut c = other.resume(&ctx()).unwrap();
    c.start(&ctx()).unwrap();
    assert_eq!(c.download("data", &ctx()).unwrap(), b"kept");
    c.close(&ctx()).unwrap();
    drop(c);
    let next = manager.acquire(spec(), &ctx()).unwrap();
    assert_eq!(next.snapshot().generation, initial.generation + 1);
    assert_ne!(next.snapshot().container, initial.container);
}
#[test]
fn incomplete_cleanup_is_reported_and_explicit_recovery_finishes_it() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    mock.0.lock().unwrap().fail_volume_remove = true;
    assert!(computer.close(&ctx()).is_err());
    assert_eq!(computer.snapshot().phase, Phase::CleanupRequired);
    drop(computer);
    assert!(mock.0.lock().unwrap().container.is_none());
    assert!(mock.0.lock().unwrap().volume.is_some());
    mock.0.lock().unwrap().fail_volume_remove = false;
    manager.recover_cleanup(&ctx()).unwrap();
    assert_eq!(manager.snapshot().unwrap().unwrap().phase, Phase::Closed);
    assert!(mock.0.lock().unwrap().volume.is_none());
}
#[test]
fn unsafe_specs_paths_and_tool_grants_are_rejected_before_dispatch() {
    let (_root, mock, manager) = setup();
    let mut unpinned = spec();
    unpinned.image = "debian:latest".into();
    assert!(manager.acquire(unpinned, &ctx()).is_err());
    let mut huge = spec();
    huge.memory_bytes = u64::MAX;
    assert!(manager.acquire(huge, &ctx()).is_err());
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    for path in ["/etc/passwd", "../outside", "dir/../../escape", ""] {
        assert!(computer.download(path, &ctx()).is_err());
    }
    assert!(
        computer
            .upload("big", &vec![0; FILE_LIMIT + 1], &ctx())
            .is_err()
    );
    assert_eq!(mock.0.lock().unwrap().execs, before);
    let mut tools = GuestTools::new(computer, GuestPolicy::default());
    assert!(!tools.definitions().iter().any(|d| d.name == "shell"));
    let call = ToolCall {
        id: "x".into(),
        name: "write_file".into(),
        arguments: json!({"path":"effect","content":"no"}),
    };
    assert!(tools.call(&call, &ctx()).unwrap().is_error);
    assert_eq!(mock.0.lock().unwrap().execs, before);
}
#[test]
fn changed_confinement_refuses_guest_operations_and_foreign_state_owner_is_rejected() {
    let (root, mock, manager) = setup();
    let mut c = manager.acquire(spec(), &ctx()).unwrap();
    mock.0
        .lock()
        .unwrap()
        .container
        .as_mut()
        .unwrap()
        .privileged = true;
    assert!(c.upload("bad", b"no", &ctx()).is_err());
    assert!(!mock.0.lock().unwrap().files.contains_key("bad"));
    c.close(&ctx()).unwrap();
    drop(c);
    let foreign = ComputerManager::from_store(
        PrivateStore::open(root.path()).unwrap(),
        "foreign",
        "pal",
        mock,
    )
    .unwrap();
    assert!(foreign.snapshot().is_err());
}
#[test]
fn create_arguments_enforce_sandbox_without_bind_mounts_or_socket() {
    let (_root, _mock, manager) = setup();
    let c = manager.acquire(spec(), &ctx()).unwrap();
    let args = namzu_computer::engine::create_arguments(c.snapshot());
    for flag in [
        "--pull=never",
        "--read-only",
        "--network=none",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges:true",
        "--user=65532:65532",
        "--restart=no",
    ] {
        assert!(args.iter().any(|s| s == flag));
    }
    assert!(!args.iter().any(|s| s.contains("type=bind")
        || s.contains("docker.sock")
        || s == "--privileged"
        || s == "--network=host"));
}

#[test]
fn failed_pre_effect_checkpoint_blocks_guest_execution() {
    let (root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    let record = root.path().join("computer.json");
    let saved = root.path().join("saved.json");
    std::fs::rename(&record, &saved).unwrap();
    std::fs::create_dir(&record).unwrap();
    assert!(computer.upload("must-not-exist", b"data", &ctx()).is_err());
    assert_eq!(mock.0.lock().unwrap().execs, before);
    std::fs::remove_dir(&record).unwrap();
    std::fs::rename(saved, record).unwrap();
    computer.close(&ctx()).unwrap();
}

#[test]
fn failed_remote_stop_is_explicitly_cleanup_required_until_recovery() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    {
        let mut state = mock.0.lock().unwrap();
        state.cancel_exec = true;
        state.fail_kill = true;
    }
    assert_eq!(
        computer.upload("uncertain", b"data", &ctx()).unwrap_err(),
        AgentError::State("remote stop failed; computer cleanup required")
    );
    assert_eq!(computer.snapshot().phase, Phase::CleanupRequired);
    assert!(mock.0.lock().unwrap().container.as_ref().unwrap().running);
    drop(computer);
    assert_eq!(
        manager.snapshot().unwrap().unwrap().phase,
        Phase::CleanupRequired
    );
    mock.0.lock().unwrap().fail_kill = false;
    manager.recover_cleanup(&ctx()).unwrap();
    assert!(mock.0.lock().unwrap().container.is_none());
    assert!(mock.0.lock().unwrap().volume.is_none());
}

#[test]
fn cancellation_during_creation_cleans_new_volume_with_fresh_cleanup_context() {
    let (_root, mock, manager) = setup();
    mock.0.lock().unwrap().cancel_volume = true;
    let context = ctx();
    assert!(matches!(
        manager.acquire(spec(), &context),
        Err(AgentError::Cancelled)
    ));
    assert!(context.remaining().is_err());
    assert!(mock.0.lock().unwrap().volume.is_none());
    assert!(mock.0.lock().unwrap().container.is_none());
    assert_eq!(manager.snapshot().unwrap().unwrap().phase, Phase::Closed);
}

#[test]
fn explicit_release_retains_healthy_running_computer_until_owned_cleanup() {
    let (_root, mock, manager) = setup();
    let computer = manager.acquire(spec(), &ctx()).unwrap();
    let tools = GuestTools::new(computer, GuestPolicy::default());
    let record = tools.into_computer().release(&ctx()).unwrap();
    assert_eq!(record.phase, Phase::Running);
    assert!(mock.0.lock().unwrap().container.as_ref().unwrap().running);
    assert!(mock.0.lock().unwrap().volume.is_some());
    let computer = manager.resume(&ctx()).unwrap();
    computer.detach(&ctx()).unwrap();
    assert!(!mock.0.lock().unwrap().container.as_ref().unwrap().running);
    manager.recover_cleanup(&ctx()).unwrap();
    assert!(mock.0.lock().unwrap().container.is_none());
    assert!(mock.0.lock().unwrap().volume.is_none());
}

#[test]
fn persistent_host_guard_preserves_files_and_stops_on_cancelled_setup_or_unwind() {
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    computer.upload("answer", b"keep", &ctx()).unwrap();
    computer.detach(&ctx()).unwrap();
    let mut computer = manager.resume(&ctx()).unwrap();
    computer.preserve_workspace_on_drop();
    let cancelled = ctx();
    cancelled.cancellation().cancel();
    assert_eq!(computer.start(&cancelled), Err(AgentError::Cancelled));
    drop(computer);
    assert!(mock.0.lock().unwrap().volume.is_some());
    let mut computer = manager.resume(&ctx()).unwrap();
    computer.preserve_workspace_on_drop();
    computer.start(&ctx()).unwrap();
    assert_eq!(computer.download("answer", &ctx()).unwrap(), b"keep");
    let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _lease = computer;
        panic!("host callback failed");
    }));
    assert!(unwind.is_err());
    let state = mock.0.lock().unwrap();
    assert!(!state.container.as_ref().unwrap().running);
    assert_eq!(state.files.get("answer").unwrap(), b"keep");
    drop(state);
    manager.recover_cleanup(&ctx()).unwrap();
}

fn blank_png(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut e = png::Encoder::new(&mut bytes, width.into(), height.into());
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()
            .unwrap()
            .write_image_data(&vec![0; usize::from(width) * usize::from(height) * 3])
            .unwrap();
    }
    bytes
}
#[test]
fn desktop_preflight_rejects_invalid_input_before_guest_exec() {
    use namzu_computer::desktop::{Request, ScreenConfig};
    let (_root, mock, manager) = setup();
    let mut s = spec();
    s.desktop = Some(ScreenConfig::default());
    let mut c = manager.acquire(s, &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    for r in [
        Request::Move { x: -1, y: 0 },
        Request::Click {
            x: 1024,
            y: 0,
            button: namzu_computer::desktop::Button::Left,
        },
        Request::Type { text: "ü".into() },
        Request::Key {
            keys: vec!["CTRL".into(), "bad".into()],
        },
        Request::Scroll {
            horizontal: 0,
            vertical: i8::MIN,
        },
    ] {
        assert!(c.desktop(r, &ctx()).is_err());
    }
    assert_eq!(mock.0.lock().unwrap().execs, before);
    assert!(c.snapshot().pending.is_none());
}
#[test]
fn desktop_requires_explicit_spec_and_no_host_fallback() {
    let (_root, mock, manager) = setup();
    let mut c = manager.acquire(spec(), &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    assert!(
        c.desktop(namzu_computer::desktop::Request::Info, &ctx())
            .is_err()
    );
    assert_eq!(mock.0.lock().unwrap().execs, before);
}
#[test]
fn desktop_info_screenshot_and_input_round_trip() {
    use namzu_computer::desktop::{Reply, Request, ScreenConfig};
    let (_root, _mock, manager) = setup();
    let mut s = spec();
    s.desktop = Some(ScreenConfig::default());
    let mut c = manager.acquire(s, &ctx()).unwrap();
    assert!(matches!(
        c.desktop(Request::Info, &ctx()).unwrap(),
        Reply::Info { width: 1024, .. }
    ));
    assert!(matches!(
        c.desktop(Request::Screenshot, &ctx()).unwrap(),
        Reply::Screenshot { .. }
    ));
    assert!(matches!(
        c.desktop(
            Request::Key {
                keys: vec!["CTRL".into(), "a".into()]
            },
            &ctx()
        )
        .unwrap(),
        Reply::Applied
    ));
    assert!(c.snapshot().pending.is_none());
}
#[test]
fn failed_input_stops_computer_and_retains_pending_without_replay() {
    use namzu_computer::desktop::{Request, ScreenConfig};
    let (_root, mock, manager) = setup();
    let mut s = spec();
    s.desktop = Some(ScreenConfig::default());
    let mut c = manager.acquire(s, &ctx()).unwrap();
    mock.0.lock().unwrap().fail_desktop = true;
    assert!(c.desktop(Request::Move { x: 10, y: 20 }, &ctx()).is_err());
    assert_eq!(c.snapshot().phase, Phase::Stopped);
    assert_eq!(c.snapshot().pending.as_ref().unwrap().kind, "desktop_input");
    mock.0.lock().unwrap().fail_desktop = false;
    c.start(&ctx()).unwrap();
    assert!(c.desktop(Request::Info, &ctx()).is_ok());
    assert!(c.desktop(Request::Move { x: 10, y: 20 }, &ctx()).is_err());
    c.acknowledge_uncertain().unwrap();
    assert!(c.desktop(Request::Move { x: 10, y: 20 }, &ctx()).is_ok());
}
#[test]
fn malformed_desktop_response_stops_and_fences_input() {
    use namzu_computer::desktop::{Request, ScreenConfig};
    let (_root, mock, manager) = setup();
    let mut s = spec();
    s.desktop = Some(ScreenConfig::default());
    let mut c = manager.acquire(s, &ctx()).unwrap();
    mock.0.lock().unwrap().bad_reply = true;
    assert!(c.desktop(Request::Move { x: 10, y: 20 }, &ctx()).is_err());
    assert_eq!(c.snapshot().phase, Phase::Stopped);
    assert!(c.snapshot().pending.is_some());
}
#[test]
fn screenshot_png_size_dimensions_and_crc_are_validated() {
    use namzu_computer::desktop::{ScreenConfig, validate_png};
    let s = ScreenConfig {
        width: 320,
        height: 240,
    };
    let bytes = blank_png(320, 240);
    assert!(validate_png(&bytes, &s).is_ok());
    assert!(validate_png(&bytes, &ScreenConfig::default()).is_err());
    let mut bad = bytes.clone();
    bad[40] ^= 255;
    assert!(validate_png(&bad, &s).is_err());
    assert!(validate_png(&vec![0; namzu_computer::desktop::SCREEN_BYTES + 1], &s).is_err());
}
#[test]
fn desktop_key_and_text_limits_preserve_literal_input_and_redact_debug() {
    use namzu_computer::desktop::{Request, ScreenConfig};
    let s = ScreenConfig::default();
    assert!(
        Request::Type {
            text: "aA!?\n\t $(literal)".into()
        }
        .validate(&s)
        .is_ok()
    );
    for r in [
        Request::Type {
            text: "x".repeat(1025),
        },
        Request::Type { text: "a\0".into() },
        Request::Key {
            keys: vec!["CTRL".into(), "CTRL".into(), "a".into()],
        },
        Request::Key {
            keys: vec!["a".into(), "CTRL".into()],
        },
    ] {
        assert!(r.validate(&s).is_err());
    }
    assert!(
        !format!(
            "{:?}",
            Request::Type {
                text: "PRIVATE_INPUT".into()
            }
        )
        .contains("PRIVATE_INPUT")
    );
}
#[test]
fn cancelled_desktop_input_stops_remote_work_and_never_replays() {
    use namzu_computer::desktop::{Request, ScreenConfig};
    let (_root, mock, manager) = setup();
    let context = ctx();
    let mut s = spec();
    s.desktop = Some(ScreenConfig::default());
    let mut c = manager.acquire(s, &context).unwrap();
    mock.0.lock().unwrap().cancel_exec = true;
    let before = mock.0.lock().unwrap().execs;
    assert!(matches!(
        c.desktop(
            Request::Click {
                x: 10,
                y: 20,
                button: namzu_computer::desktop::Button::Left
            },
            &context
        ),
        Err(AgentError::Cancelled)
    ));
    assert_eq!(c.snapshot().phase, Phase::Stopped);
    assert_eq!(c.snapshot().pending.as_ref().unwrap().kind, "desktop_input");
    assert!(c.desktop(Request::Move { x: 10, y: 20 }, &context).is_err());
    assert_eq!(mock.0.lock().unwrap().execs, before + 1);
}
#[test]
fn legacy_specs_are_headless_and_desktop_arguments_are_explicit() {
    let old = serde_json::to_value(spec()).unwrap();
    assert!(old.get("desktop").is_none());
    assert!(
        serde_json::from_value::<ComputerSpec>(old)
            .unwrap()
            .desktop
            .is_none()
    );
    let (_root, _mock, manager) = setup();
    let mut s = spec();
    s.desktop = Some(namzu_computer::desktop::ScreenConfig {
        width: 800,
        height: 600,
    });
    let c = manager.acquire(s, &ctx()).unwrap();
    let args = namzu_computer::engine::create_arguments(c.snapshot());
    assert!(args.iter().any(|s| s == "NAMZU_DESKTOP_SCREEN=800x600x24"));
}

fn browser_spec() -> ComputerSpec {
    let mut s = spec();
    s.desktop = Some(Default::default());
    s.browser = true;
    s.memory_bytes = 768 * 1024 * 1024;
    s.pids = 128;
    s
}
#[test]
fn browser_is_opt_in_and_invalid_navigation_has_no_guest_effect() {
    use namzu_computer::browser::Request;
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(spec(), &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    assert!(computer.browser(Request::Info, &ctx()).is_err());
    assert_eq!(mock.0.lock().unwrap().execs, before);
    computer.close(&ctx()).unwrap();
    drop(computer);
    let mut computer = manager.acquire(browser_spec(), &ctx()).unwrap();
    let before = mock.0.lock().unwrap().execs;
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "data:text/html,hello",
        "chrome://settings",
        "https://user:secret@example.com/",
        "http://host/\nsecret",
    ] {
        assert!(
            computer
                .browser(Request::Navigate { url: url.into() }, &ctx())
                .is_err()
        );
    }
    assert_eq!(mock.0.lock().unwrap().execs, before);
    assert!(computer.snapshot().pending.is_none());
}
#[test]
fn browser_navigation_and_inspection_roundtrip_without_pending() {
    use namzu_computer::browser::{Reply, Request};
    let (_root, _mock, manager) = setup();
    let mut computer = manager.acquire(browser_spec(), &ctx()).unwrap();
    assert!(matches!(
        computer.browser(Request::Info, &ctx()).unwrap(),
        Reply::Info { .. }
    ));
    assert!(matches!(
        computer
            .browser(
                Request::Navigate {
                    url: "https://example.com/path".into()
                },
                &ctx()
            )
            .unwrap(),
        Reply::Navigated { .. }
    ));
    assert!(matches!(
        computer.browser(Request::Snapshot, &ctx()).unwrap(),
        Reply::Snapshot { .. }
    ));
    assert!(computer.snapshot().pending.is_none());
}
#[test]
fn failed_navigation_stops_and_inspection_preserves_fence() {
    use namzu_computer::browser::Request;
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(browser_spec(), &ctx()).unwrap();
    mock.0.lock().unwrap().fail_browser = true;
    assert!(
        computer
            .browser(
                Request::Navigate {
                    url: "https://example.com/".into()
                },
                &ctx()
            )
            .is_err()
    );
    assert_eq!(computer.snapshot().phase, Phase::Stopped);
    assert_eq!(
        computer.snapshot().pending.as_ref().unwrap().kind,
        "browser_navigate"
    );
    mock.0.lock().unwrap().fail_browser = false;
    computer.start(&ctx()).unwrap();
    computer.browser(Request::Snapshot, &ctx()).unwrap();
    assert_eq!(
        computer.snapshot().pending.as_ref().unwrap().kind,
        "browser_navigate"
    );
    let before = mock.0.lock().unwrap().execs;
    assert!(
        computer
            .browser(
                Request::Navigate {
                    url: "about:blank".into()
                },
                &ctx()
            )
            .is_err()
    );
    assert_eq!(mock.0.lock().unwrap().execs, before);
    computer.acknowledge_uncertain().unwrap();
    computer
        .browser(
            Request::Navigate {
                url: "about:blank".into(),
            },
            &ctx(),
        )
        .unwrap();
}
#[test]
fn cancelled_browser_navigation_and_wrong_kind_are_never_replayed() {
    use namzu_computer::browser::Request;
    let (_root, mock, manager) = setup();
    let mut computer = manager.acquire(browser_spec(), &ctx()).unwrap();
    mock.0.lock().unwrap().cancel_exec = true;
    assert!(
        computer
            .browser(
                Request::Navigate {
                    url: "about:blank".into()
                },
                &ctx()
            )
            .is_err()
    );
    assert_eq!(computer.snapshot().phase, Phase::Stopped);
    assert!(computer.snapshot().pending.is_some());
    computer.acknowledge_uncertain().unwrap();
    computer.start(&ctx()).unwrap();
    mock.0.lock().unwrap().wrong_browser = true;
    assert!(computer.browser(Request::Snapshot, &ctx()).is_err());
    assert_eq!(computer.snapshot().phase, Phase::Stopped);
    assert_eq!(
        computer.snapshot().pending.as_ref().unwrap().kind,
        "browser_snapshot"
    );
}
#[test]
fn browser_spec_requires_resources_and_does_not_change_legacy_images() {
    let mut s = browser_spec();
    s.desktop = None;
    assert!(s.validate().is_err());
    s = browser_spec();
    s.memory_bytes = 256 * 1024 * 1024;
    assert!(s.validate().is_err());
    s = browser_spec();
    s.pids = 64;
    assert!(s.validate().is_err());
    let legacy: ComputerSpec = serde_json::from_value(
        json!({"image":spec().image,"memory_bytes":268435456,"cpu_millis":1000,"pids":64}),
    )
    .unwrap();
    assert!(!legacy.browser);
    let (_root, _mock, manager) = setup();
    let computer = manager.acquire(browser_spec(), &ctx()).unwrap();
    let args = namzu_computer::engine::create_arguments(computer.snapshot());
    assert!(args.iter().any(|v| v == "NAMZU_BROWSER=1"));
    assert!(args.iter().any(|v| v.contains("size=268435456")));
    assert!(args.iter().any(|v| v == "--network=none"));
}
#[test]
fn browser_reply_rejects_oversize_data_and_debug_redacts_urls() {
    use namzu_computer::browser::{Page, Reply, Request};
    let r = Request::Navigate {
        url: "https://example.com/?secret=private".into(),
    };
    assert!(!format!("{r:?}").contains("private"));
    let mut page = Page {
        url: "about:blank".into(),
        title: "x".repeat(2049),
        frame_id: "f".into(),
        loader_id: "l".into(),
    };
    assert!(Reply::Info { page }.validate().is_err());
    page = Page {
        url: "about:blank".into(),
        title: "fixture".into(),
        frame_id: String::new(),
        loader_id: "l".into(),
    };
    assert!(Reply::Navigated { page }.validate().is_err());
    assert!(
        serde_json::from_value::<Request>(
            json!({"action":"navigate","url":"about:blank","evaluate":"secret"})
        )
        .is_err()
    );
}
