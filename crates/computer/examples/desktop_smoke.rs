//! Real owned Docker desktop acceptance; no host display or model quota.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use namzu_computer::{
    ComputerManager, ComputerSpec, Docker,
    desktop::{Button, Reply, Request, ScreenConfig},
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
        "desktop-acceptance",
        "oracle",
        Docker::local("/usr/local/bin/docker")?,
    )?;
    let c = ctx();
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut spec = ComputerSpec::new(&args[2]);
        spec.desktop = Some(ScreenConfig::default());
        let mut computer = manager.acquire(spec, &c)?;
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if computer.desktop(Request::Info, &c).is_ok() {
                break;
            }
            if Instant::now() > until {
                return Err("X server not ready".into());
            }
        }
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
                "/usr/local/bin/desktop-fixture",
            ])
            .status()?;
        assert!(status.success());
        let until = Instant::now() + Duration::from_secs(10);
        while computer.download("gui-ready", &c).is_err() {
            if Instant::now() > until {
                return Err("GUI fixture not ready".into());
            }
        }
        let Reply::Screenshot { png_base64, .. } = computer.desktop(Request::Screenshot, &c)?
        else {
            panic!()
        };
        let png = STANDARD.decode(png_base64)?;
        assert_eq!(pixel(&png, 30, 40), vec![0x22, 0x44, 0x66]);
        std::fs::write(root.join("screen-before.png"), &png)?;
        assert!(computer.desktop(Request::Move { x: -1, y: 0 }, &c).is_err());
        assert!(computer.snapshot().pending.is_none());
        assert!(
            computer
                .desktop(
                    Request::Type {
                        text: "unsupported ü".into()
                    },
                    &c
                )
                .is_err()
        );
        assert!(computer.snapshot().pending.is_none());
        computer.desktop(Request::Move { x: 100, y: 100 }, &c)?;
        computer.desktop(
            Request::Click {
                x: 100,
                y: 100,
                button: Button::Left,
            },
            &c,
        )?;
        computer.desktop(
            Request::Type {
                text: "Ab9!?".into(),
            },
            &c,
        )?;
        computer.desktop(
            Request::Key {
                keys: vec!["CTRL".into(), "a".into()],
            },
            &c,
        )?;
        computer.desktop(
            Request::Scroll {
                horizontal: 1,
                vertical: -2,
            },
            &c,
        )?;
        computer.desktop(
            Request::Drag {
                from_x: 100,
                from_y: 100,
                to_x: 200,
                to_y: 150,
            },
            &c,
        )?;
        let Reply::Info {
            pointer_x,
            pointer_y,
            ..
        } = computer.desktop(Request::Info, &c)?
        else {
            panic!()
        };
        assert_eq!((pointer_x, pointer_y), (200, 150));
        let until = Instant::now() + Duration::from_secs(10);
        let events = loop {
            let text = String::from_utf8(computer.download("gui-events.jsonl", &c)?)?;
            let events: Vec<serde_json::Value> = text
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            if b"Ab9!?".iter().all(|k| {
                events
                    .iter()
                    .any(|e| e["event"] == "key" && e["keysym"] == u64::from(*k))
            }) && events.iter().any(|e| {
                e["event"] == "key"
                    && e["keysym"] == u64::from(b'a')
                    && e["state"].as_u64().is_some_and(|s| s & 4 != 0)
            }) && events
                .iter()
                .any(|e| e["event"] == "button" && e["detail"] == 7)
            {
                break events;
            }
            if Instant::now() > until {
                return Err("keyboard oracle incomplete".into());
            }
        };
        let keys: Vec<u64> = events
            .iter()
            .filter(|e| e["event"] == "key")
            .filter_map(|e| e["keysym"].as_u64())
            .collect();
        for k in *b"Ab9!?" {
            assert!(keys.contains(&u64::from(k)));
        }
        assert!(events.iter().any(|e| e["event"] == "key"
            && e["keysym"] == u64::from(b'a')
            && e["state"].as_u64().unwrap() & 4 != 0));
        assert!(
            events
                .iter()
                .any(|e| e["event"] == "button" && e["detail"] == 4)
        );
        assert!(
            events
                .iter()
                .any(|e| e["event"] == "button" && e["detail"] == 7)
        );
        let Reply::Screenshot { png_base64, .. } = computer.desktop(Request::Screenshot, &c)?
        else {
            panic!()
        };
        let png = STANDARD.decode(png_base64)?;
        assert_eq!(pixel(&png, 30, 40), vec![0xaa, 0x33, 0x22]);
        std::fs::write(root.join("screen-after.png"), &png)?;
        assert!(computer.snapshot().pending.is_none());
        computer.stop(&c)?;
        assert!(computer.desktop(Request::Info, &c).is_err());
        computer.start(&c)?;
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if computer.desktop(Request::Info, &c).is_ok() {
                break;
            }
            if Instant::now() > until {
                return Err("restarted desktop unavailable".into());
            }
        }

        computer.close(&c)?;
        Ok(())
    })();
    manager.recover_cleanup(&ctx())?;
    result?;
    println!("{{\"desktop_pixels_input_oracles_and_cleanup\":\"passed\",\"live_model_calls\":0}}");
    Ok(())
}
