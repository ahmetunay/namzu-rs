# Namzu Rust reconstruction

Read README.md and ROADMAP.md before continuing. This is a new implementation;
the old cloud workspace source was not recovered. Never attribute its historical
test results or completed modules to this tree.

- Keep core independent of the CLI, provider and optional capability packages.
- Do not use live model credentials for deterministic tests. Loopback fixtures
  and ScriptedProvider suffice. Never log/read stored keys to discover values.
- Default turn timeout is 300 seconds, per the user's earlier instruction.
- Validate tool admission, batch structure, cancellation and budgets before effects.
- Test actual behavior and boundary failures; avoid wall-clock timing races.
- Run cargo fmt --all -- --check, cargo test --workspace --locked and
  cargo clippy --workspace --all-targets --locked -- -D warnings for Rust changes.
- Commit validated milestones, then run python3 scripts/checkpoint.py. Verify
  the bundle's restore result. Retain Cargo.lock and the upstream license.
- Local commits/bundles are not an external backup. Record a remote backup as
  verified only after uploading to the user-selected repository and verifying
  the remote tree/commit. GitHub connection alone is not backup.
- Update ROADMAP.md with only what has been reimplemented and tested here.
- The user explicitly authorized publishing to the PUBLIC repository
  https://github.com/ahmetunay/namzu-rs and asked to upload EVERY completed
  milestone in this project. This authorization persists across turns; do not
  ask again merely because the target repository is public.
- Before ending a turn that changes this project: validate the change, commit
  all intended source/docs, push main to origin and verify that remote main
  matches local HEAD. Follow BACKUP.md. Never force-push to overwrite work.
  If upload fails, report the unbacked-up commit and actual failure. Never
  upload credentials, .env, generated target/artifacts or private conversations.
