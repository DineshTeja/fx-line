use std::{env, process::ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(output) => {
            if let Some(output) = output {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("fx-agent: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<Option<String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "run".into());

    match command.as_str() {
        "run" if args.next().is_none() => {
            fx_line::agent::run_daemon()?;
            Ok(None)
        }
        "plan" => {
            let request = request(args)?;
            Ok(Some(serde_json::to_string(&fx_line::agent::plan(
                &request,
            )?)?))
        }
        "request" => {
            let request = request(args)?;
            Ok(Some(fx_line::agent::run_request(&request)?))
        }
        "install" if args.next().is_none() => {
            fx_line::agent::install(&env::current_exe()?)?;
            Ok(Some("fx-agent is running".into()))
        }
        "uninstall" if args.next().is_none() => {
            fx_line::agent::uninstall()?;
            Ok(Some("fx-agent was removed".into()))
        }
        "status" if args.next().is_none() => Ok(Some(if fx_line::agent::is_running()? {
            "running".into()
        } else {
            "stopped".into()
        })),
        _ => Err(
            "usage: fx-agent [run|install|uninstall|status|plan REQUEST|request REQUEST]".into(),
        ),
    }
}

fn request(args: impl Iterator<Item = String>) -> Result<String, &'static str> {
    let request = args.collect::<Vec<_>>().join(" ");
    if request.trim().is_empty() {
        Err("request cannot be empty")
    } else {
        Ok(request)
    }
}
