//! Run the action a preset chose.
//!
//! The program is executed directly, never through a shell, and nothing from the state is put in
//! its arguments. The state is written to its stdin. Its stdout and stderr are copied to the
//! writers `run` was given as they arrive, or collected for `--json`. The status is the action's:
//! its exit code, or 128 + N when signal N killed it.
//!
//! When our stdout closes, the copy stops and the pipe is dropped, so an action that keeps writing
//! gets the same `SIGPIPE` it would have got writing to the closed pipe itself.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use crate::exit::Failure;

/// Bytes read from a pipe at a time.
const CHUNK: usize = 8192;

/// What to run, and what it gets.
pub(crate) struct Action<'a> {
    /// The action's name, for messages.
    pub(crate) name: &'a str,
    /// The program, already resolved against the preset's directory.
    pub(crate) program: PathBuf,
    /// Its arguments.
    pub(crate) args: &'a [String],
    /// Written to its stdin, then stdin is closed.
    pub(crate) stdin: Vec<u8>,
    /// `Some` replaces the inherited environment. `None` inherits the process environment.
    pub(crate) env: Option<&'a BTreeMap<String, String>>,
    /// Set on top of the environment.
    pub(crate) extra_env: Vec<(&'static str, String)>,
}

/// An action that ran with its output collected.
pub(crate) struct Collected {
    /// The action's status.
    pub(crate) status: u8,
    /// Everything it wrote to stdout.
    pub(crate) stdout: Vec<u8>,
    /// Everything it wrote to stderr.
    pub(crate) stderr: Vec<u8>,
}

/// Run `action`, copying its stdout to `stdout` and its stderr to `stderr` as they arrive.
///
/// A failure to write our stdout, other than a closed pipe, is reported on stderr after the action
/// ends. The status is still the action's: it has already run.
pub(crate) fn stream<O: Write, E: Write + Send + 'static>(
    action: Action<'_>,
    stdout: &mut O,
    stderr: &Arc<Mutex<E>>,
) -> Result<u8, Failure> {
    let mut child = spawn(&action)?;
    let feeder = feed(&mut child, action.stdin);
    let err_pipe = child.stderr.take();
    let sink = Arc::clone(stderr);
    let copier = thread::spawn(move || {
        if let Some(mut pipe) = err_pipe {
            copy_chunks(&mut pipe, |chunk| {
                let mut guard = sink.lock().unwrap_or_else(PoisonError::into_inner);
                guard.write_all(chunk).and_then(|()| guard.flush())
            });
        }
    });
    let mut write_error = None;
    if let Some(mut pipe) = child.stdout.take() {
        write_error = copy_chunks(&mut pipe, |chunk| stdout.write_all(chunk).and_then(|()| stdout.flush()));
    }
    drop(copier.join());
    let status = child.wait().map_err(|err| Failure::Io(format!("cannot wait for action `{}`: {err}", action.name)));
    drop(feeder.join());
    if let Some(err) = write_error.filter(|err| err.kind() != ErrorKind::BrokenPipe) {
        let mut guard = stderr.lock().unwrap_or_else(PoisonError::into_inner);
        drop(writeln!(guard, "jev: cannot write stdout: {err}"));
        drop(guard.flush());
    }
    Ok(status_code(status?))
}

/// Run `action` and collect its output.
pub(crate) fn collect(action: Action<'_>) -> Result<Collected, Failure> {
    let mut child = spawn(&action)?;
    let feeder = feed(&mut child, action.stdin);
    let output = child
        .wait_with_output()
        .map_err(|err| Failure::Io(format!("cannot wait for action `{}`: {err}", action.name)))?;
    drop(feeder.join());
    Ok(Collected { status: status_code(output.status), stdout: output.stdout, stderr: output.stderr })
}

fn spawn(action: &Action<'_>) -> Result<Child, Failure> {
    let mut command = Command::new(&action.program);
    command.args(action.args);
    if let Some(env) = action.env {
        command.env_clear().envs(env);
    }
    command.envs(action.extra_env.iter().map(|(key, value)| (key, value)));
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    command.spawn().map_err(|err| {
        Failure::Io(format!("cannot run action `{}` ({}): {err}", action.name, action.program.display()))
    })
}

/// Write the state on a thread of its own, so an action that writes before it reads cannot
/// deadlock against us. An action that exits without reading makes the write fail, which is fine.
fn feed(child: &mut Child, bytes: Vec<u8>) -> thread::JoinHandle<()> {
    let pipe = child.stdin.take();
    thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            drop(pipe.write_all(&bytes));
        }
    })
}

/// Read `pipe` to the end, handing each chunk to `write`. The first write error stops the copy and
/// is returned; the pipe is then dropped by the caller.
fn copy_chunks(pipe: &mut impl Read, mut write: impl FnMut(&[u8]) -> std::io::Result<()>) -> Option<std::io::Error> {
    let mut buf = vec![0_u8; CHUNK];
    loop {
        let read = match pipe.read(&mut buf) {
            Ok(0) => return None,
            Ok(read) => read,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return None,
        };
        if let Some(chunk) = buf.get(..read)
            && let Err(err) = write(chunk)
        {
            return Some(err);
        }
    }
}

/// The exit code, or 128 + the signal that killed the process.
fn status_code(status: ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return u8::try_from(code).unwrap_or(u8::MAX);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return u8::try_from(128 + signal).unwrap_or(u8::MAX);
        }
    }
    u8::MAX
}
