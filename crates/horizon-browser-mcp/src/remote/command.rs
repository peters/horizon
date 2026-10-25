//! The remote command lines of the bridge: the probe that reads the record of
//! the remote profile host, the MCP server of that host, and the file calls of
//! the two rewrites.
//!
//! Each line is one argument of `ssh`, which joins the arguments after the
//! target with spaces and gives the result to the login shell of the remote
//! machine. Every value that this module puts in a line is therefore either
//! quoted for that shell or a fixed word of this module, and no value is ever
//! added as a separate `ssh` argument.

use std::fmt::Write as _;
use std::path::Path;

/// The remote default runtime root of a profile host, as the remote shell
/// reads it. Horizon never sets a root, so this is what a person gets.
const DEFAULT_ROOT: &str = "\"$HOME/.horizon\"";

/// The remote server of a profile host, with the identity that the bridge
/// forwards.
///
/// The actor is the local identity of the agent panel, which the host stamps its
/// pages with. The host instance is the one the remote host wrote in its record,
/// never the instance of this computer.
pub(super) fn server_command(program: &str, actor: &str, host_instance: &str, root: Option<&Path>) -> String {
    let mut command = format!(
        "env HORIZON_BROWSER_ACTOR={} HORIZON_BROWSER_HOST_INSTANCE={}",
        quote(actor),
        quote(host_instance)
    );
    if let Some(root) = root {
        let _ = write!(command, " HORIZON_BROWSER_ROOT={}", quote(&path_text(root)));
    }
    let _ = write!(command, " {} mcp --connect --inline-screenshots", quote(program));
    command
}

/// Prints the runtime root of the remote host, then its record on one line.
///
/// A missing record ends the probe with exit code 3, which says that no host
/// runs on that runtime root. The probe reads no profile, because the host
/// decides which profiles it serves.
pub(super) fn probe_command(root: Option<&Path>) -> String {
    format!(
        "root={root}; printf '%s\\n' \"$root\"; tr -d '\\n' < \"$root/profile-host.json\" || exit 3; \
         printf '\\n'",
        root = root_expression(root)
    )
}

/// One shell word for the runtime root of the remote host.
pub(super) fn root_expression(root: Option<&Path>) -> String {
    root.map_or_else(|| DEFAULT_ROOT.to_string(), |root| quote(&path_text(root)))
}

/// Read one file of the remote host. `cat` streams the bytes, so a large file is
/// never held twice.
pub(super) fn read_file_command(path: &Path) -> String {
    format!("cat {}", quote(&path_text(path)))
}

/// Stage one file into `directory`, from the standard input of `ssh`. The copy
/// takes its final name only after every byte arrived, so an interrupted
/// transfer leaves a `.part` name that the cleanup of the call removes.
pub(super) fn stage_file_command(directory: &str, subdirectory: usize, name: &str) -> String {
    let folder = quote(&format!("{directory}/{subdirectory}"));
    let staged = quote(&format!("{directory}/{subdirectory}/{name}"));
    format!("set -eu; mkdir -p -- {folder} && cat > {staged}.part && mv -- {staged}.part {staged}")
}

/// Make the staging folder of the runtime root, then the private folder of one
/// call. The call folder must not exist, so an interrupted call can not hand its
/// copies to the next one.
pub(super) fn create_directory_command(parent: &str, directory: &str) -> String {
    format!(
        "set -eu; umask 077; mkdir -p -- {parent} && mkdir -- {directory}",
        parent = quote(parent),
        directory = quote(directory)
    )
}

/// Remove the staging directory of one call and every staged copy in it. `--`
/// keeps a folder name that begins with a dash a name and not an option.
pub(super) fn remove_directory_command(directory: &str) -> String {
    format!("rm -rf -- {}", quote(directory))
}

/// A value as the remote shell must read it: one word, with no expansion.
pub(super) fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        create_directory_command, probe_command, quote, read_file_command, remove_directory_command, root_expression,
        server_command, stage_file_command,
    };

    #[test]
    fn the_remote_server_names_the_forwarded_identity_and_the_capture_flags() {
        let command = server_command("horizon-browser", "horizon:panel-1", "host-instance", None);
        assert_eq!(
            command,
            "env HORIZON_BROWSER_ACTOR='horizon:panel-1' HORIZON_BROWSER_HOST_INSTANCE='host-instance' \
             'horizon-browser' mcp --connect --inline-screenshots"
        );
        assert!(
            !command.contains("HORIZON_BROWSER_ROOT"),
            "an unset root leaves the remote default in place"
        );
        let named = server_command(
            "horizon-browser",
            "horizon:panel-1",
            "host-instance",
            Some(Path::new("/srv/browser state")),
        );
        assert!(
            named.starts_with(
                "env HORIZON_BROWSER_ACTOR='horizon:panel-1' HORIZON_BROWSER_HOST_INSTANCE='host-instance' \
                HORIZON_BROWSER_ROOT='/srv/browser state'"
            ),
            "a root with a space stays one word: {named}"
        );
    }

    #[test]
    fn the_probe_reads_the_record_of_the_remote_root_and_no_profile_folder() {
        assert_eq!(root_expression(None), "\"$HOME/.horizon\"");
        assert_eq!(
            probe_command(None),
            "root=\"$HOME/.horizon\"; printf '%s\\n' \"$root\"; \
             tr -d '\\n' < \"$root/profile-host.json\" || exit 3; printf '\\n'"
        );
        assert_eq!(
            probe_command(Some(Path::new("/srv/browser state"))),
            "root='/srv/browser state'; printf '%s\\n' \"$root\"; \
             tr -d '\\n' < \"$root/profile-host.json\" || exit 3; printf '\\n'"
        );
        assert!(
            !probe_command(None).contains("browser-profiles"),
            "the bridge takes no profile list from the folders of the remote machine"
        );
    }

    #[test]
    fn a_remote_path_with_a_quote_can_not_leave_its_word() {
        assert_eq!(quote("/tmp/a'b"), "'/tmp/a'\\''b'");
        assert_eq!(read_file_command(Path::new("/tmp/a'b")), "cat '/tmp/a'\\''b'");
        assert_eq!(remove_directory_command("/tmp/x"), "rm -rf -- '/tmp/x'");
        assert_eq!(
            create_directory_command("/tmp/staging", "/tmp/staging/x"),
            "set -eu; umask 077; mkdir -p -- '/tmp/staging' && mkdir -- '/tmp/staging/x'"
        );
    }

    #[test]
    fn a_staged_file_arrives_under_its_own_name_inside_the_call_folder() {
        let command = stage_file_command("/srv/state/browser-staging/call-1", 2, "a b.pdf");
        assert_eq!(
            command,
            "set -eu; mkdir -p -- '/srv/state/browser-staging/call-1/2' && \
             cat > '/srv/state/browser-staging/call-1/2/a b.pdf'.part && \
             mv -- '/srv/state/browser-staging/call-1/2/a b.pdf'.part '/srv/state/browser-staging/call-1/2/a b.pdf'"
        );
        assert_eq!(
            stage_file_command("/srv/state/browser-staging/call-1", 0, "a;rm -rf /"),
            "set -eu; mkdir -p -- '/srv/state/browser-staging/call-1/0' && \
             cat > '/srv/state/browser-staging/call-1/0/a;rm -rf /'.part && \
             mv -- '/srv/state/browser-staging/call-1/0/a;rm -rf /'.part '/srv/state/browser-staging/call-1/0/a;rm -rf /'",
            "a file name stays one quoted word"
        );
    }

    #[test]
    fn a_staging_path_under_a_root_with_a_space_stays_one_word() {
        let directory = "/srv/browser state/browser-staging/call-1";
        assert_eq!(
            root_expression(Some(Path::new("/srv/browser state"))),
            "'/srv/browser state'"
        );
        assert_eq!(
            read_file_command(Path::new("/srv/browser state/captures/record.webm")),
            "cat '/srv/browser state/captures/record.webm'"
        );
        assert_eq!(
            stage_file_command(directory, 0, "report.pdf"),
            "set -eu; mkdir -p -- '/srv/browser state/browser-staging/call-1/0' && \
             cat > '/srv/browser state/browser-staging/call-1/0/report.pdf'.part && \
             mv -- '/srv/browser state/browser-staging/call-1/0/report.pdf'.part \
             '/srv/browser state/browser-staging/call-1/0/report.pdf'"
        );
        assert_eq!(
            remove_directory_command(directory),
            "rm -rf -- '/srv/browser state/browser-staging/call-1'"
        );
    }

    #[test]
    fn a_staging_path_under_a_root_with_a_quote_stays_one_word() {
        let directory = "/srv/peter's browser/browser-staging/call-1";
        assert_eq!(
            create_directory_command("/srv/peter's browser/browser-staging", directory),
            "set -eu; umask 077; mkdir -p -- '/srv/peter'\\''s browser/browser-staging' && \
             mkdir -- '/srv/peter'\\''s browser/browser-staging/call-1'"
        );
        assert_eq!(
            stage_file_command(directory, 1, "report.pdf"),
            "set -eu; mkdir -p -- '/srv/peter'\\''s browser/browser-staging/call-1/1' && \
             cat > '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'.part && \
             mv -- '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'.part \
             '/srv/peter'\\''s browser/browser-staging/call-1/1/report.pdf'"
        );
        assert_eq!(
            remove_directory_command(directory),
            "rm -rf -- '/srv/peter'\\''s browser/browser-staging/call-1'"
        );
    }

    #[test]
    fn a_staging_path_that_begins_with_a_dash_is_not_an_option() {
        let directory = "-staging/call-1";
        assert_eq!(root_expression(Some(Path::new("-root"))), "'-root'");
        assert_eq!(
            create_directory_command("-staging", directory),
            "set -eu; umask 077; mkdir -p -- '-staging' && mkdir -- '-staging/call-1'"
        );
        assert_eq!(
            stage_file_command(directory, 0, "report.pdf"),
            "set -eu; mkdir -p -- '-staging/call-1/0' && \
             cat > '-staging/call-1/0/report.pdf'.part && \
             mv -- '-staging/call-1/0/report.pdf'.part '-staging/call-1/0/report.pdf'"
        );
        assert_eq!(remove_directory_command(directory), "rm -rf -- '-staging/call-1'");
        assert_eq!(remove_directory_command("-rf"), "rm -rf -- '-rf'");
    }
}
