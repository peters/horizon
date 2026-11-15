//! The one relay command that Horizon writes on a machine.
//!
//! An agent starts its MCP server as a child of one command, and that command
//! serves every session of the user: it learns the `tmux` session that it runs in,
//! finds the socket of that session and copies its input and output to it.
//! `socat` relays, `nc -U` next, and a script of the standard library of `python3`
//! last. The setup finds the program with [`PROBE_COMMAND`]; a machine with none
//! of them gets no entry and the answer [`super::RELAY_MISSING`].

use horizon_browser_control::manifest::block_machines;

use super::REMOTE_DIR;

/// The one command that the setup runs on a machine to find its relay program.
pub const PROBE_COMMAND: &str = "command -v socat || command -v nc || command -v python3";

/// The program of the relay of a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayProgram {
    Socat,
    Nc,
    Python,
}

impl RelayProgram {
    /// The name that the probe found, which is the command of the entry.
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            Self::Socat => "socat",
            Self::Nc => "nc",
            Self::Python => "python3",
        }
    }
}

/// The program that the answer of [`PROBE_COMMAND`] names, by the order of that
/// command: `socat` first, then `nc`, then `python3`.
#[must_use]
pub fn probe_outcome(answer: &str) -> Option<RelayProgram> {
    let found = answer.lines().find_map(|line| {
        let path = line.trim();
        let program = path.rsplit('/').next()?;
        match program {
            "socat" => Some(RelayProgram::Socat),
            "nc" => Some(RelayProgram::Nc),
            "python3" => Some(RelayProgram::Python),
            _ => None,
        }
    })?;
    Some(found)
}

/// The command and the arguments of the MCP entry on a machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelayCommand {
    pub command: String,
    pub args: Vec<String>,
}

/// The script of the third relay: the standard library of `python3` copies both
/// directions of one Unix socket stream. It holds no single quote, so it passes
/// a shell unchanged.
pub const PYTHON_RELAY: &str = "\
import socket,sys,threading
s=socket.socket(socket.AF_UNIX);s.connect(sys.argv[1])
def r():
 while 1:
  b=s.recv(65536)
  if not b:break
  sys.stdout.buffer.write(b);sys.stdout.buffer.flush()
threading.Thread(target=r,daemon=True).start()
while 1:
 b=sys.stdin.buffer.read1(65536)
 if not b:break
 s.sendall(b)";

/// The relay command of `machine` for `program`: it makes the folder of the
/// sockets again, reads its own `tmux` session and relays through that session's
/// socket. Outside a `tmux` session it ends with status 1 and no connection.
///
/// # Errors
/// Describes the name rule that the machine name broke.
pub fn relay_command(machine: &str, program: RelayProgram) -> Result<RelayCommand, super::MachineRefusal> {
    let machine = block_machines::check_machine_name(machine)?;
    let directory = format!("{REMOTE_DIR}/{machine}");
    let socket = format!("\"$d/$s.sock\"");
    let relay = match program {
        RelayProgram::Socat => format!("exec socat - UNIX-CONNECT:{socket}"),
        RelayProgram::Nc => format!("exec nc -U {socket}"),
        RelayProgram::Python => format!("exec python3 -c \"{PYTHON_RELAY}\" {socket}"),
    };
    let line = format!(
        "d={directory}; s=$(tmux display-message -p '#S') || exit 1; mkdir -p \"$d\" && chmod 700 \"$d\" || exit 1; {relay}"
    );
    Ok(RelayCommand {
        command: "sh".to_owned(),
        args: vec!["-c".to_owned(), line],
    })
}
