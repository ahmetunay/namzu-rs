//! Actual Chromium DOM/AX/navigation and X11 screen oracle in an owned guest.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_computer::{
    ComputerManager, ComputerSpec, Docker, Phase,
    browser::{Reply, Request},
    desktop,
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
fn pixel(png: &[u8], x: usize, y: usize) -> Vec<u8> {
    let mut r = png::Decoder::new(png).read_info().unwrap();
    let mut b = vec![0; r.output_buffer_size()];
    let f = r.next_frame(&mut b).unwrap();
    let offset = (y * f.width as usize + x) * 3;
    b[offset..offset + 3].to_vec()
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let root = Path::new(&args[1]);
    let manager = ComputerManager::from_store(
        PrivateStore::open(root)?,
        "browser-acceptance",
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
        eprintln!("browser acceptance: initial info");
        let Reply::Info { page } = computer.browser(Request::Info, &c)? else {
            panic!()
        };
        assert_eq!(page.url, "about:blank");
        let status = std::process::Command::new("/usr/local/bin/docker")
            .env_remove("DOCKER_HOST")
            .env_remove("DOCKER_CONTEXT")
            .env_remove("DOCKER_TLS")
            .env_remove("DOCKER_TLS_VERIFY")
            .env_remove("DOCKER_CERT_PATH")
            .args([
                "--host=unix:///var/run/docker.sock",
                "exec",
                "--detach",
                &computer.snapshot().container,
                "/usr/local/bin/browser-fixture",
            ])
            .status()?;
        assert!(status.success());
        let until = Instant::now() + Duration::from_secs(10);
        while computer.download("browser-ready", &c).is_err() {
            if Instant::now() > until {
                return Err("browser fixture not ready".into());
            }
        }
        eprintln!("browser acceptance: initial navigation");
        let Reply::Navigated { page } = computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/".into(),
            },
            &c,
        )?
        else {
            panic!()
        };
        assert_eq!(page.title, "Namzu Browser Fixture");
        eprintln!("browser acceptance: DOM and accessibility");
        let snapshot = computer.browser(Request::Snapshot, &c)?;
        let Reply::Snapshot {
            dom,
            accessibility,
            truncated,
            ..
        } = &snapshot
        else {
            panic!()
        };
        assert!(!truncated);
        assert!(dom.iter().any(|n| n.name == "H1"));
        assert!(dom.iter().any(|n| n.text == "Kontrol paneli"));
        assert!(
            dom.iter()
                .any(|n| n.attributes.iter().any(|(k, v)| k == "id" && v == "name"))
        );
        assert!(
            accessibility
                .iter()
                .any(|n| n.role == "button" && n.name == "Run task")
        );
        assert!(
            accessibility
                .iter()
                .any(|n| n.role == "textbox" && n.name == "Your name" && n.value == "initial")
        );
        std::fs::write(
            root.join("browser-snapshot.json"),
            serde_json::to_vec_pretty(&snapshot)?,
        )?;
        let until = Instant::now() + Duration::from_secs(10);
        eprintln!("browser acceptance: screen paint");
        loop {
            let desktop::Reply::Screenshot { png_base64, .. } =
                computer.desktop(desktop::Request::Screenshot, &c)?
            else {
                panic!()
            };
            let png = STANDARD.decode(png_base64)?;
            if pixel(&png, 900, 600) == [51, 102, 153] {
                std::fs::write(root.join("browser-screen.png"), png)?;
                break;
            }
            if Instant::now() > until {
                return Err("browser paint not observed".into());
            }
        }
        eprintln!("browser acceptance: redirect and fragment");
        let Reply::Navigated { page } = computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/redirect".into(),
            },
            &c,
        )?
        else {
            panic!()
        };
        assert_eq!(page.url, "http://127.0.0.1:8765/next");
        assert_eq!(page.title, "Second Page");
        eprintln!("browser acceptance: same-document fragment");
        let Reply::Navigated { page } = computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/next#section".into(),
            },
            &c,
        )?
        else {
            panic!()
        };
        assert!(page.url.ends_with("#section"));
        eprintln!("browser acceptance: large snapshot");
        computer.browser(
            Request::Navigate {
                url: "http://127.0.0.1:8765/large".into(),
            },
            &c,
        )?;
        let Reply::Snapshot { truncated, dom, .. } = computer.browser(Request::Snapshot, &c)?
        else {
            panic!()
        };
        assert!(truncated);
        assert_eq!(dom.len(), 256);
        eprintln!("browser acceptance: rejected URL and failed navigation fence");
        assert!(
            computer
                .browser(
                    Request::Navigate {
                        url: "file:///etc/passwd".into()
                    },
                    &c
                )
                .is_err()
        );
        assert!(computer.snapshot().pending.is_none());
        assert!(
            computer
                .browser(
                    Request::Navigate {
                        url: "http://127.0.0.1:9/".into()
                    },
                    &c
                )
                .is_err()
        );
        assert_eq!(computer.snapshot().phase, Phase::Stopped);
        assert_eq!(
            computer.snapshot().pending.as_ref().unwrap().kind,
            "browser_navigate"
        );
        computer.start(&c)?;
        computer.browser(Request::Info, &c)?;
        assert!(computer.snapshot().pending.is_some());
        assert!(
            computer
                .browser(
                    Request::Navigate {
                        url: "about:blank".into()
                    },
                    &c
                )
                .is_err()
        );
        computer.acknowledge_uncertain()?;
        computer.browser(
            Request::Navigate {
                url: "about:blank".into(),
            },
            &c,
        )?;
        computer.close(&c)?;
        Ok(())
    })();
    manager.recover_cleanup(&ctx())?;
    result?;
    println!(
        "{{\"chromium_navigation_dom_ax_screen_and_fence\":\"passed\",\"live_model_calls\":0}}"
    );
    Ok(())
}
