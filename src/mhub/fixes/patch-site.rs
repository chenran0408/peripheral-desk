//! Build helper for the release packager's staging copy of M HUB.
#[path = "g75_patch.rs"]
mod g75;

use std::{env, fs, path::PathBuf, process::ExitCode};

fn run() -> Result<(), String> {
    let root = env::args_os().nth(1).ok_or("Usage: patch-site <site>")?;
    let path = PathBuf::from(root).join(g75::MODULE);
    let source = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !source.starts_with(g75::STAMP) {
        let target = g75::patched(&source).ok_or("G75 remapping component was not recognized")?;
        fs::write(path, target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
