mod fx;
mod output;

use std::{env, error::Error, io, io::Write, process::ExitCode};

type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fx-line: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let request = args
        .next()
        .ok_or_else(|| io::Error::other("usage: fx-line <request> [cwd] [current-line]"))?;
    let cwd = args
        .next()
        .unwrap_or(env::current_dir()?.to_string_lossy().into_owned());
    let current_line = args.next().unwrap_or_default();

    if args.next().is_some() {
        return Err(io::Error::other("too many arguments").into());
    }

    let command = fx::generate(&request, &cwd, &current_line)?;
    write!(io::stdout().lock(), "{command}")?;
    Ok(())
}
