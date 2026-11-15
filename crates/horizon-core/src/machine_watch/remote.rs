//! The text that one watch runs on its machine, and the whole `ssh` command line of
//! that watch.
//!
//! [`REMOTE_TEXT`] is one constant of this build: POSIX shell that runs on `dash`, on
//! `bash` and on the shell of `BusyBox`, and that uses `tmux`, `ps` and the built-ins
//! of that shell. No value of a record and no value of a report takes part in it.
//!
//! `sshd` runs the remote command with the login shell of the user of that machine, so
//! the command of the watch is the one word `sh` and [`super::reader`] writes the text to
//! the standard input of the child: a login shell that is not POSIX runs it all the same.
//!
//! The text prints one report and then one line per interval: a still machine prints
//! one short heartbeat line instead of its report. Each field of a report is one line
//! of words, so no value that a program or a person put into a session name, a folder
//! name, a title or a program name can break a line. The format and its bounds are in
//! [`super::report`].
//!
//! The text asks `tmux` with `-u`: an `ssh` command of a machine often has no UTF-8
//! locale, and such a client of `tmux` prints `_` in the place of every character
//! outside ASCII, which would drop the braille frame of the title of a Codex pane.
//!
//! `HORIZON_MACHINE_WATCH_TMUX_SOCKET` is unset in real use; a test sets it, so the
//! same text talks to a private `tmux` server instead of the server of the person.

use crate::machines::{MachineError, Machines};
use crate::ssh::{MachineKeys, SshConnection};

/// The text that the watch runs on one machine.
///
/// It reads the panes of every `tmux` session of that user. A machine with no
/// `tmux`, or with no `tmux` server, says that in one line and repeats it at the
/// slower interval.
pub const REMOTE_TEXT: &str = r#"V=1
FAST=2
SLOW=30
# The tmux of the watch: the private server of a test, or the person's own, and -u
# because the client of an ssh command often has no UTF-8 locale.
tm() { tmux -u ${HORIZON_MACHINE_WATCH_TMUX_SOCKET:+-L "$HORIZON_MACHINE_WATCH_TMUX_SOCKET" -f /dev/null} "$@"; }
# The last processes of the terminal `1`, newest first: the program of a pane whose
# foreground process is an interpreter such as node. The pane is `p` of `pane`.
terminal() {
  case $1 in /dev/*) ;; *) return 0 ;; esac
  ps -t "$1" -o args= 2>/dev/null | {
    a=; b=; c=
    while IFS= read -r l; do a=$b; b=$c; c=$l; done
    for v in "$c" "$b" "$a"; do
      [ -n "$v" ] || continue
      set -f; set -- $v; set +f
      printf 'A\t%s\t%s\n' "$p" "$*"
    done
  }
}
# One pane: session, program, folder, title, the command line of an interpreter
# that runs there, and the last six rows of its screen.
pane() {
  p=$1
  s=$(tm display-message -p -t "$p" '#{session_name}' 2>/dev/null) || return 0
  c=$(tm display-message -p -t "$p" '#{pane_current_command}' 2>/dev/null)
  d=$(tm display-message -p -t "$p" '#{pane_current_path}' 2>/dev/null)
  t=$(tm display-message -p -t "$p" '#{pane_title}' 2>/dev/null)
  set -f
  set -- $s; s=$*; set -- $c; c=$*; set -- $d; d=$*; set -- $t; t=$*
  set +f
  printf 'P\t%s\t%s\t%s\t%s\t%s\n' "$p" "$s" "$c" "$d" "$t"
  # The name of a program has no fixed case: the tmux of macOS names the python of a
  # panel `Python`, and the list below holds the lower-case names.
  lc=$(printf '%s' "$c" | tr 'A-Z' 'a-z')
  case " sh bash dash zsh ksh fish node nodejs deno bun python python3 ruby perl php java env " in
    *" $lc "*) terminal "$(tm display-message -p -t "$p" '#{pane_tty}' 2>/dev/null)" ;;
  esac
  tm capture-pane -p -t "$p" 2>/dev/null | {
    r1=; r2=; r3=; r4=; r5=; r6=
    while IFS= read -r l; do
      set -f; set -- $l; set +f; r1=$r2; r2=$r3; r3=$r4; r4=$r5; r5=$r6; r6=$*
    done
    for v in "$r1" "$r2" "$r3" "$r4" "$r5" "$r6"; do [ -n "$v" ] && printf 'R\t%s\t%s\n' "$p" "$v"; done
  }
}
# One report: a header, one block for each pane of each session of this user, and an
# end. A machine with no tmux or no tmux server says so in one line.
report() {
  printf 'HZ\t%s\n' "$V"
  rc=0
  if ! command -v tmux >/dev/null 2>&1; then
    printf 'X\tno-tmux\n'; rc=1
  elif panes=$(tm list-panes -a -F '#{pane_id}' 2>/dev/null); then
    i=0
    for p in $panes; do
      case $p in %*) ;; *) continue ;; esac
      i=$((i + 1))
      [ "$i" -le 64 ] || break
      pane "$p"
    done
  else
    printf 'X\tno-server\n'; rc=1
  fi
  printf 'Z\n'
  return "$rc"
}
last=
while :; do
  now=$(report) && step=$FAST || step=$SLOW
  if [ "$now" = "$last" ]; then printf 'B\n'; else printf '%s\n' "$now"; last=$now; fi
  sleep "$step"
done
"#;

/// The one word of the remote command: `sh` reads [`REMOTE_TEXT`] on its standard input.
pub const REMOTE_PROGRAM: &str = "sh";

/// One machine that the watch reaches: its name and the whole `ssh` command line of
/// the watch of that machine.
///
/// The command line holds what the login of a terminal of that machine holds: the
/// route of the record, the connect bound and the keepalives of a machine terminal,
/// and, when the record binds a machine key, that key and its known-hosts file. It
/// adds `BatchMode=yes`, so a machine that asks for a password fails the attempt
/// instead, and it asks for no terminal, so it holds no `-t`. The last argument is
/// [`REMOTE_PROGRAM`], and [`REMOTE_TEXT`] reaches that shell on the input of the
/// child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchTarget {
    /// The name of the machine in the machine list.
    pub machine: String,
    /// The argv of the `ssh` child, without the program name.
    pub args: Vec<String>,
}

impl WatchTarget {
    /// The watch of the machine `name` of `machines`, with the machine key that
    /// this computer binds to it.
    ///
    /// # Errors
    /// One sentence for a machine that the list does not hold, and the refusal of
    /// the record when its route no longer passes the rule of an SSH terminal.
    pub fn of(machines: &Machines, keys: &MachineKeys, name: &str) -> Result<Self, MachineError> {
        let record = machines.find(name).ok_or_else(|| MachineError::Unavailable {
            reason: format!("The machine list of this computer has no machine {name}."),
        })?;
        // The route passes the rule of an SSH terminal, as it does for a terminal
        // of that machine. The watch is no terminal: it takes no `-t`, because its
        // report is a pipe and not a screen.
        let mut terminal = record.checked_terminal()?;
        terminal.remote_command = None;
        let connection = match machines.key(&record.name) {
            Some(key) => SshConnection::for_agent_terminal_with_key(&terminal, &keys.binding(key)),
            None => SshConnection::for_agent_terminal(&terminal),
        }
        .as_machine_terminal();
        // The login of a machine terminal, in the form that a child with its own
        // bounds takes: `BatchMode=yes`, so a machine that asks for a password or a
        // passphrase fails an attempt instead of waiting for a person who is not
        // there. The login shell of the machine runs one word, and the text of the
        // watch arrives on the input of the child.
        let mut args = connection.ssh_transport_args();
        args.push(REMOTE_PROGRAM.to_owned());
        Ok(Self {
            machine: record.name.clone(),
            args,
        })
    }
}
