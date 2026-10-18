use namzu_core::{Cancellation, TurnContext};
use namzu_tools::process::{ProcessSpec, run_process_input};
use std::{collections::BTreeMap, time::Duration};
fn spec(command: &str) -> ProcessSpec {
    ProcessSpec {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), command.into()],
        cwd: "/".into(),
        env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        timeout: Duration::from_secs(10),
        output_limit: 1024 * 1024,
    }
}
#[test]
fn simultaneous_large_input_and_early_output_cannot_deadlock() {
    let input = vec![b'y'; 256 * 1024];
    let context = TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap();
    let result = run_process_input(
        spec("head -c 131072 /dev/zero | tr '\\000' 'x'; cat"),
        &context,
        &input,
    )
    .unwrap();
    assert_eq!(result.status, "exited");
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.len(), 384 * 1024);
    assert!(
        result.stdout.as_bytes()[..128 * 1024]
            .iter()
            .all(|b| *b == b'x')
    );
    assert_eq!(&result.stdout.as_bytes()[128 * 1024..], &input);
}
#[test]
fn closed_stdin_does_not_prevent_collecting_completed_output() {
    let context = TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap();
    let result = run_process_input(
        spec("exec 0<&-; printf done"),
        &context,
        &vec![b'x'; 256 * 1024],
    )
    .unwrap();
    assert_eq!(result.stdout, "done");
    assert_eq!(result.exit_code, Some(0));
}

#[test]
fn explicit_large_output_budget_supports_desktop_frames() {
    let context = TurnContext::new(Duration::from_secs(20), Cancellation::default()).unwrap();
    let mut process = spec("head -c 5242880 /dev/zero | tr '\\000' 'x'");
    process.output_limit = 12 * 1024 * 1024;
    let result = run_process_input(process, &context, b"").unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert_eq!(result.stdout.len(), 5 * 1024 * 1024);
    assert!(result.stdout.bytes().all(|byte| byte == b'x'));
}
