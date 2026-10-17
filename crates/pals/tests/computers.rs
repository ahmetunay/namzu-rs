use namzu_computer::Docker;
use namzu_pals::{PalCreate, Registry};
#[test]
fn computer_binding_checks_owner_and_keeps_metadata_outside_pal_workspace() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let registry = Registry::open(&state, root.path().join("workspaces")).unwrap();
    let pal = registry
        .create(PalCreate {
            owner: "owner".into(),
            name: "Pal".into(),
            purpose: String::new(),
            route: "fixture".into(),
            tools: None,
        })
        .unwrap();
    assert!(
        registry
            .computers(
                "foreign",
                &pal.id,
                Docker::local("/not-executed/docker").unwrap()
            )
            .is_err()
    );
    let manager = registry
        .computers(
            "owner",
            &pal.id,
            Docker::local("/not-executed/docker").unwrap(),
        )
        .unwrap();
    assert!(manager.snapshot().unwrap().is_none());
    assert!(state.join(&pal.id).join("computers").is_dir());
    assert!(!pal.workspace.join("computers").exists());
}
