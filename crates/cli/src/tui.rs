use crate::execute::{RunOptions, execute_observed};
use clap::Args;
use namzu_core::Cancellation;

#[derive(Args)]
pub struct TuiArgs {
    #[command(flatten)]
    pub options: RunOptions,
    /// Offline scripted tool replay in a temporary workspace; no model/account.
    #[arg(long, conflicts_with = "resume")]
    pub demo: bool,
}

pub fn run(args: TuiArgs, cancellation: Cancellation) -> std::io::Result<()> {
    let settings = namzu_tui::Settings {
        title: if args.demo {
            "DEMO · scripted fixture, answer 12".into()
        } else {
            format!(
                "{} · {}",
                args.options
                    .provider
                    .as_deref()
                    .unwrap_or("configured provider"),
                args.options.model.as_deref().unwrap_or("configured model")
            )
        },
        workspace: if args.demo {
            "temporary demo workspace".into()
        } else {
            args.options.cwd.display().to_string()
        },
        permissions: if args.demo {
            "demo read/write".into()
        } else {
            format!(
                "read{}{}",
                if args.options.allow_write {
                    "/write"
                } else {
                    "-only"
                },
                if args.options.allow_shell {
                    " + HOST SHELL"
                } else {
                    ""
                }
            )
        },
        session_id: args.options.resume.clone(),
    };
    namzu_tui::run(
        settings,
        cancellation,
        move |prompt, session_id, signal, events| {
            if args.demo {
                namzu_tui::Outcome {
                    session_id: None,
                    error: crate::offline_demo_observed(signal, events).err(),
                }
            } else {
                let mut options = args.options.clone();
                options.resume = session_id;
                let execution = execute_observed(prompt, options, signal, events);
                namzu_tui::Outcome {
                    session_id: execution.session_id,
                    error: execution.result.err().map(|e| e.to_string()),
                }
            }
        },
    )
}
