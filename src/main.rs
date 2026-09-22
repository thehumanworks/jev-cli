//! Process entry point. All behaviour lives in the library so tests can drive it in memory.

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdin_is_terminal = stdin.is_terminal();
    let stdout = io::stdout();
    let mut io = jev::Io {
        args: std::env::args_os().collect(),
        stdin,
        stdout: stdout.lock(),
        stderr: Arc::new(Mutex::new(io::stderr())),
        stdin_is_terminal,
        env: None,
    };
    ExitCode::from(jev::run(&mut io, None))
}
