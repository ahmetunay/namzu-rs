//! Full CLI -> private registry -> real loopback provider -> SDK/tools acceptance.
//! All credentials are fixture values; normal tests never use account quota/Docker.
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
struct Fixture {
    address: String,
    calls: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    server: Option<thread::JoinHandle<()>>,
}
impl Fixture {
    fn new(f: impl Fn(&Value) -> Value + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}/v1", listener.local_addr().unwrap());
        let calls = Arc::new(Mutex::new(vec![]));
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, shutdown) = (calls.clone(), stop.clone());
        let f = Arc::new(f);
        let server = thread::spawn(move || {
            let mut jobs = vec![];
            while !shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        let requests = requests.clone();
                        let f = f.clone();
                        jobs.push(thread::spawn(move||{
                    let v=request(&mut socket);requests.lock().unwrap().push(v.clone());let body=f(&v).to_string();
                    let _=write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",body.len(),body);
                }));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
            for job in jobs {
                job.join().unwrap();
            }
        });
        Self {
            address,
            calls,
            stop,
            server: Some(server),
        }
    }
    fn command(&self, state: &std::path::Path) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_namzu-rs"));
        c.args(["pal", "--state-dir"])
            .arg(state)
            .args([
                "--model",
                "fixture",
                "--base-url",
                &self.address,
                "--api-key-env",
                "NAMZU_PAL_FIXTURE_KEY",
                "--timeout",
                "10",
            ])
            .env("NAMZU_PAL_FIXTURE_KEY", "fixture-only")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        c
    }
    fn run(&self, state: &std::path::Path, args: &[&str]) -> Value {
        let out = self.command(state).args(args).output().unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        value(out)["result"].clone()
    }
    fn fail(&self, state: &std::path::Path, args: &[&str]) -> Value {
        let out = self.command(state).args(args).output().unwrap();
        assert!(!out.status.success());
        value(out)
    }
    fn create(&self, state: &std::path::Path, name: &str, write: bool) -> String {
        let mut args = vec![];
        if write {
            args.push("--allow-write");
        }
        args.extend(["create", name]);
        self.run(state, &args)["id"].as_str().unwrap().into()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.server.take().unwrap().join().unwrap();
    }
}
fn request(s: &mut TcpStream) -> Value {
    s.set_read_timeout(Some(Duration::from_secs(12))).unwrap();
    let mut header = vec![];
    while !header.ends_with(b"\r\n\r\n") {
        let mut b = [0];
        s.read_exact(&mut b).unwrap();
        header.push(b[0]);
        assert!(header.len() < 16384);
    }
    let h = String::from_utf8(header).unwrap();
    assert!(
        h.to_lowercase()
            .contains("authorization: bearer fixture-only\r\n")
    );
    let len: usize = h
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .map(|(_, v)| v.trim().parse().unwrap())
        })
        .unwrap();
    assert!(len < 8 * 1024 * 1024);
    let mut body = vec![0; len];
    s.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
fn value(out: Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| {
        panic!(
            "bad CLI JSON: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    })
}
fn text(v: &str) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"role":"assistant","content":v}}]})
}
fn tool(name: &str, args: Value) -> Value {
    json!({"choices":[{"finish_reason":"tool_calls","message":{"role":"assistant","content":null,"tool_calls":[{"id":"fixture-tool","type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]})
}
#[test]
fn profile_and_invocation_permissions_intersect_and_followups_fork_immutable_history() {
    let f = Fixture::new(|v| {
        let messages = v["messages"].as_array().unwrap();
        if messages.last().unwrap()["role"] == "user" {
            {
                let mut reply = tool("write_file", json!({"path":"answer.txt","content":"42\n"}));
                reply["choices"][0]["message"]["tool_calls"][0]["id"] =
                    json!(format!("write-{}", messages.len()));
                reply
            }
        } else {
            text("PAL_DONE")
        }
    });
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let id = f.create(&state, "writer", true);
    let first = f.run(&state, &["ask", &id, "write answer"]);
    let task = first["task_id"].as_str().unwrap();
    let p = f.run(&state, &["show", &id]);
    let workspace = std::path::Path::new(p["workspace"].as_str().unwrap());
    assert!(!workspace.join("answer.txt").exists());
    let saved = f.run(&state, &["inspect", &id, task]);
    assert!(
        saved["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool" && m["is_error"] == true)
    );
    let next = f.run(
        &state,
        &["--allow-write", "ask", &id, "now write", "--follow", task],
    );
    assert_eq!(
        std::fs::read_to_string(workspace.join("answer.txt")).unwrap(),
        "42\n"
    );
    assert_ne!(first["task_id"], next["task_id"]);
    assert_eq!(f.run(&state, &["inspect", &id, task]), saved);
    let calls = f.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert!(
        calls[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["function"]["name"] != "write_file")
    );
    assert!(calls[2]["messages"].as_array().unwrap().len() > 4);
    drop(calls);
    let mismatch = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["pal", "--state-dir"])
        .arg(&state)
        .args(["--model", "other", "ask", &id, "route mismatch"])
        .output()
        .unwrap();
    assert!(!mismatch.status.success());
    assert!(
        value(mismatch)["error"]
            .as_str()
            .unwrap()
            .contains("pinned Pal route")
    );
    f.fail(&state, &["--owner", "other", "show", &id]);
    assert_eq!(f.run(&state, &["tasks", &id]).as_array().unwrap().len(), 2);
    assert_eq!(f.calls.lock().unwrap().len(), 4);
}
#[test]
fn registry_ceiling_denies_write_even_when_current_invocation_grants_it() {
    let f = Fixture::new(|v| {
        if v["messages"].as_array().unwrap().last().unwrap()["role"] == "user" {
            tool("write_file", json!({"path":"forbidden","content":"no"}))
        } else {
            text("DENIED")
        }
    });
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let id = f.create(&state, "reader", false);
    f.run(&state, &["--allow-write", "ask", &id, "write"]);
    let p = f.run(&state, &["show", &id]);
    assert!(
        !std::path::Path::new(p["workspace"].as_str().unwrap())
            .join("forbidden")
            .exists()
    );
    assert!(
        f.calls.lock().unwrap()[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["function"]["name"] != "write_file")
    );
}
#[test]
fn mailbox_cli_deduplicates_delivers_fifo_across_processes_and_never_replays() {
    let f = Fixture::new(|v| text(v["messages"][0]["content"].as_str().unwrap()));
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let id = f.create(&state, "inbox", false);
    let first = f.run(&state, &["send", &id, "m-one", "FIRST"]);
    assert_eq!(first["inserted"], true);
    let duplicate = f.run(&state, &["send", &id, "m-one", "FIRST"]);
    assert_eq!(duplicate["inserted"], false);
    assert_eq!(first["task_id"], duplicate["task_id"]);
    f.fail(&state, &["send", &id, "m-one", "different"]);
    f.run(&state, &["send", &id, "m-two", "SECOND"]);
    let batch = f.run(&state, &["work", &id, "--max-messages", "2"]);
    assert_eq!(batch["deliveries"][0]["result"]["output"], "FIRST");
    assert_eq!(batch["deliveries"][1]["result"]["output"], "SECOND");
    assert_eq!(f.calls.lock().unwrap().len(), 2);
    assert!(
        f.run(&state, &["work", &id])["deliveries"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.calls.lock().unwrap().len(), 2);
    let tasks = f.run(&state, &["tasks", &id]);
    assert!(
        tasks
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["status"] == "complete")
    );
    f.fail(&state, &["run", &id, first["task_id"].as_str().unwrap()]);
    f.run(&state, &["status", &id, "--revision", "1", "paused"]);
    f.fail(&state, &["ask", &id, "blocked"]);
    assert_eq!(f.calls.lock().unwrap().len(), 2);
}
#[test]
fn persistent_team_runs_parallel_with_ordered_results_and_shared_allocation() {
    let gate = Arc::new((Mutex::new(0usize), std::sync::Condvar::new()));
    let g = gate.clone();
    let f = Fixture::new(move |_| {
        let (lock, cv) = &*g;
        let mut n = lock.lock().unwrap();
        *n += 1;
        cv.notify_all();
        if *n == 1 {
            let (value, wait) = cv
                .wait_timeout_while(n, Duration::from_secs(6), |n| *n < 2)
                .unwrap();
            n = value;
            assert!(
                !wait.timed_out(),
                "two providers were never active together"
            );
        }
        drop(n);
        text("TEAM_DONE")
    });
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let ids: Vec<_> = (0..6)
        .map(|i| f.create(&state, &format!("pal{i}"), false))
        .collect();
    let mut args = vec!["--max-steps", "12", "--max-tool-calls", "6", "team"];
    args.extend(ids.iter().map(String::as_str));
    args.extend(["--prompt", "work", "--concurrency", "3"]);
    let result = f.run(&state, &args);
    assert_eq!(result["allocation_per_pal"]["steps"], 2);
    assert_eq!(result["allocation_per_pal"]["tool_calls"], 1);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(result["results"][i]["pal_id"], *id);
        assert_eq!(result["results"][i]["output"], "TEAM_DONE");
        assert_eq!(f.run(&state, &["tasks", id])[0]["status"], "complete");
    }
    assert_eq!(f.calls.lock().unwrap().len(), 6);
    f.fail(
        &state,
        &[
            "--max-steps",
            "1",
            "team",
            &ids[0],
            &ids[1],
            "--prompt",
            "budget",
        ],
    );
    f.fail(&state, &["team", &ids[0], &ids[0], "--prompt", "duplicate"]);
    assert_eq!(f.calls.lock().unwrap().len(), 6);
    assert_eq!(
        f.run(&state, &["tasks", &ids[0]]).as_array().unwrap().len(),
        1
    );
}
#[test]
fn missing_guest_computer_never_falls_back_to_host_and_failed_task_cannot_resume() {
    let f = Fixture::new(|_| panic!("no inference allowed"));
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let p = f.run(&state, &["--guest", "create", "guest"]);
    let id = p["id"].as_str().unwrap();
    let error = f.fail(
        &state,
        &["--docker", "/missing-docker", "ask", id, "do work"],
    );
    assert_eq!(error["status"], "error");
    let tasks = f.run(&state, &["tasks", id]);
    assert_eq!(tasks[0]["status"], "interrupted");
    f.fail(
        &state,
        &[
            "ask",
            id,
            "no replay",
            "--follow",
            tasks[0]["id"].as_str().unwrap(),
        ],
    );
    assert!(f.calls.lock().unwrap().is_empty());
}
#[test]
fn pal_tui_rejects_pipe_before_private_state_or_provider_setup() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("new-state");
    let output = Command::new(env!("CARGO_BIN_EXE_namzu-rs"))
        .args(["pal", "--state-dir"])
        .arg(&state)
        .args(["tui", "p-missing"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!state.exists());
}
