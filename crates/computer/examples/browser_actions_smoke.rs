//! Real page-input and binary-file oracles; no model or host browser.
use namzu_computer::{
    ComputerManager, ComputerSpec, Docker, Phase,
    browser::{Document, Reply, Request, Target},
};
use namzu_core::{Cancellation, TurnContext};
use namzu_session::PrivateStore;
use std::{
    path::Path,
    time::{Duration, Instant},
};
fn ctx() -> TurnContext {
    TurnContext::new(Duration::from_secs(300), Cancellation::default()).unwrap()
}
fn docker() -> std::process::Command {
    let mut c = std::process::Command::new("/usr/local/bin/docker");
    for k in [
        "DOCKER_HOST",
        "DOCKER_CONTEXT",
        "DOCKER_TLS",
        "DOCKER_TLS_VERIFY",
        "DOCKER_CERT_PATH",
    ] {
        c.env_remove(k);
    }
    c.arg("--host=unix:///var/run/docker.sock");
    c
}
fn fixture(
    computer: &mut namzu_computer::Computer<'_>,
    c: &TurnContext,
) -> Result<(), Box<dyn std::error::Error>> {
    computer.browser(Request::Info, c)?;
    assert!(
        docker()
            .args([
                "exec",
                "--detach",
                &computer.snapshot().container,
                "/usr/local/bin/browser-fixture"
            ])
            .status()?
            .success()
    );
    let until = Instant::now() + Duration::from_secs(10);
    while computer.download("browser-ready", c).is_err() {
        if Instant::now() > until {
            return Err("fixture readiness".into());
        }
    }
    Ok(())
}
fn snapshot(
    computer: &mut namzu_computer::Computer<'_>,
    c: &TurnContext,
) -> Result<Reply, Box<dyn std::error::Error>> {
    Ok(computer.browser(Request::Snapshot, c)?)
}
fn target(snapshot: &Reply, id: &str) -> Target {
    let Reply::Snapshot { page, dom, .. } = snapshot else {
        panic!()
    };
    Target {
        document: Document::from(page),
        backend_node_id: dom
            .iter()
            .find(|n| n.attributes.iter().any(|(k, v)| k == "id" && v == id))
            .unwrap()
            .backend_node_id,
    }
}
fn text(
    computer: &mut namzu_computer::Computer<'_>,
    expected: &str,
    c: &TurnContext,
) -> Result<(), Box<dyn std::error::Error>> {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let Reply::Snapshot { dom, .. } = snapshot(computer, c)? else {
            panic!()
        };
        if dom.iter().any(|n| n.text == expected) {
            return Ok(());
        }
        if Instant::now() > until {
            return Err("page result oracle".into());
        }
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let manager = ComputerManager::from_store(
        PrivateStore::open(root)?,
        "browser-actions-acceptance",
        "oracle",
        Docker::local("/usr/local/bin/docker")?,
    )?;
    let c = ctx();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut spec = ComputerSpec::new(&args[2]);
        spec.desktop = Some(Default::default());
        spec.browser = true;
        spec.memory_bytes = 768 * 1024 * 1024;
        spec.pids = 128;
        let mut computer = manager.acquire(spec, &c)?;
        fixture(&mut computer, &c)?;
        computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/actions".into(),
            },
            &c,
        )?;
        let before = snapshot(&mut computer, &c)?;
        eprintln!("actions acceptance: fill, select, click, scroll and key");
        computer.browser(
            Request::Fill {
                target: target(&before, "edit"),
                text: "Namzu Türkçe ✓".into(),
            },
            &c,
        )?;
        computer.browser(
            Request::Select {
                target: target(&before, "choice"),
                values: vec!["b".into()],
            },
            &c,
        )?;
        text(&mut computer, "choice:b", &c)?;
        computer.browser(
            Request::Click {
                target: target(&before, "run"),
            },
            &c,
        )?;
        text(&mut computer, "clicked:Namzu Türkçe ✓", &c)?;
        computer.browser(
            Request::ScrollIntoView {
                target: target(&before, "bottom"),
            },
            &c,
        )?;
        computer.browser(
            Request::Click {
                target: target(&before, "bottom"),
            },
            &c,
        )?;
        text(&mut computer, "bottom", &c)?;
        computer.browser(
            Request::Press {
                target: target(&before, "edit"),
                key: "ENTER".into(),
            },
            &c,
        )?;
        text(&mut computer, "submitted", &c)?;
        eprintln!("actions acceptance: upload bytes and lazy form read");
        let payload = vec![0, 255, 10, 13, 42];
        computer.upload("payload.bin", &payload, &c)?;
        computer.browser(
            Request::Upload {
                target: target(&before, "file"),
                path: "payload.bin".into(),
            },
            &c,
        )?;
        computer.browser(
            Request::Click {
                target: target(&before, "send"),
            },
            &c,
        )?;
        text(&mut computer, "uploaded:5", &c)?;
        assert_eq!(computer.download("upload-received.bin", &c)?, payload);
        eprintln!("actions acceptance: bounded download and filename isolation");
        let Reply::Downloaded { bytes, path } = computer.browser(
            Request::Download {
                document: target(&before, "edit").document,
                url: "http://127.0.0.1:8765/download".into(),
                path: "saved.bin".into(),
            },
            &c,
        )?
        else {
            panic!()
        };
        assert_eq!(path, "saved.bin");
        assert_eq!(bytes, 5);
        assert_eq!(computer.download("saved.bin", &c)?, payload);
        std::fs::write(root.join("browser43-downloaded.bin"), &payload)?;
        assert!(!computer.list(".", &c)?.iter().any(|s| s == "untrusted.bin"));
        eprintln!("actions acceptance: disabled fieldset rejection");
        assert!(
            computer
                .browser(
                    Request::Fill {
                        target: target(&before, "blocked"),
                        text: "forbidden".into()
                    },
                    &c
                )
                .is_err()
        );
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "browser_action"
        );
        computer.acknowledge_uncertain()?;
        computer.start(&c)?;
        fixture(&mut computer, &c)?;
        eprintln!("actions acceptance: stale target rejection");
        computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/next".into(),
            },
            &c,
        )?;
        assert!(
            computer
                .browser(
                    Request::Click {
                        target: target(&before, "run")
                    },
                    &c
                )
                .is_err()
        );
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "browser_action"
        );
        computer.acknowledge_uncertain()?;
        computer.start(&c)?;
        fixture(&mut computer, &c)?;
        let Reply::Info { page } = computer.browser(Request::Info, &c)? else {
            panic!()
        };
        eprintln!("actions acceptance: oversized download rejection");
        assert!(
            computer
                .browser(
                    Request::Download {
                        document: Document::from(&page),
                        url: "http://127.0.0.1:8765/large-download".into(),
                        path: "large.bin".into()
                    },
                    &c
                )
                .is_err()
        );
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "browser_download"
        );
        computer.start(&c)?;
        assert!(computer.download("large.bin", &c).is_err());
        computer.browser(Request::Info, &c)?;
        assert!(computer.snapshot().pending.is_some());
        computer.acknowledge_uncertain()?;
        fixture(&mut computer, &c)?;
        let Reply::Info { page } = computer.browser(Request::Info, &c)? else {
            panic!()
        };
        eprintln!("actions acceptance: actual in-flight cancellation");
        let cancel = Cancellation::default();
        let cancelling = cancel.clone();
        let container = computer.snapshot().container.clone();
        let watcher = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                if docker()
                    .args([
                        "exec",
                        &container,
                        "/usr/bin/test",
                        "-f",
                        "/workspace/download-started",
                    ])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
                {
                    cancelling.cancel();
                    return true;
                }
                if Instant::now() > until {
                    cancelling.cancel();
                    return false;
                }
            }
        });
        let cancelled = computer.browser(
            Request::Download {
                document: Document::from(&page),
                url: "http://127.0.0.1:8765/slow-download".into(),
                path: "partial.bin".into(),
            },
            &TurnContext::new(Duration::from_secs(300), cancel)?,
        );
        assert!(watcher.join().unwrap());
        assert!(cancelled.is_err());
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "browser_download"
        );
        computer.start(&c)?;
        assert!(computer.download("partial.bin", &c).is_err());
        assert_eq!(computer.download("download-started", &c)?, b"started");
        computer.browser(Request::Info, &c)?;
        assert!(computer.snapshot().pending.is_some());
        computer.close(&c)?;
        Ok(())
    })();
    manager.recover_cleanup(&ctx())?;
    result?;
    println!(
        "{{\"page_actions_upload_download_limits_stale_target_and_cancellation\":\"passed\",\"live_model_calls\":0}}"
    );
    Ok(())
}
