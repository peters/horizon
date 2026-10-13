//! Local files dropped on a cloud agent or shell panel, sent to its worker session.
//!
//! The controller's SSH identity writes each drop into a new folder
//! `/workspace/.horizon-drops/SESSION/BATCH`. That identity owns `.horizon-drops`, the
//! session folder and the batch folder, so an agent that runs as `horizon-agent` can read
//! the files but cannot choose or redirect where the controller writes. A file is complete
//! or absent: the worker removes a file whose size differs from the local file.
use super::{Error, Result, settings::Settings, ssh::Connection, state::cloud_directory};
use horizon_cloud::{Worker, valid_id};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const WORKSPACE: &str = "/workspace";
/// The longest file name, in bytes, that the worker's file systems take.
const NAME_MAX: usize = 255;
/// Hex digits of a batch folder name: enough that two drops never pick the same one.
const BATCH_DIGITS: usize = 12;

/// Prepares the batch folder. Refuses a session the worker does not have, and a drops
/// or session folder that is a symbolic link or that another user owns, such as one an
/// agent made before agent isolation was turned on.
const PREPARE: &str = r#"
set -eu
umask 077
drops="$1/.horizon-drops"
session=$2
batch=$3
if [ ! -d "$1/sessions/$session" ]; then
    echo 'The worker has no session for this panel. Reconnect the cloud, then drop the files again.' >&2
    exit 3
fi
group=$(id -g)
if [ -f /run/horizon-tailnet/agent-isolation ]; then group=$(id -g horizon-agent); fi
owner=$(id -u)
for dir in "$drops" "$drops/$session"; do
    if [ -L "$dir" ]; then
        echo "Horizon did not send the files: $dir on the worker is a symbolic link." >&2
        exit 3
    fi
    mkdir -p -- "$dir"
    if [ "$(stat -c %u -- "$dir")" != "$owner" ]; then
        echo "Horizon did not send the files: another user owns $dir on the worker." >&2
        exit 3
    fi
    chgrp -- "$group" "$dir"
    chmod 0750 -- "$dir"
done
mkdir -- "$drops/$session/$batch"
chgrp -- "$group" "$drops/$session/$batch"
chmod 0750 -- "$drops/$session/$batch"
"#;

/// Writes one file from standard input. It never replaces a file, and it removes a file
/// that is shorter or longer than the local file, such as after a cancelled transfer.
const WRITE: &str = r#"
set -eu
umask 077
dir="$1/.horizon-drops/$2/$3"
name=$4
size=$5
group=$(id -g)
if [ -f /run/horizon-tailnet/agent-isolation ]; then group=$(id -g horizon-agent); fi
if [ -L "$dir" ] || [ ! -d "$dir" ]; then
    echo 'The upload folder on the worker is missing. Drop the files again.' >&2
    exit 3
fi
file="$dir/$name"
set -C
cat > "$file"
received=$(stat -c %s -- "$file")
if [ "$received" != "$size" ]; then
    rm -f -- "$file"
    echo "The worker received $received of $size bytes of $name, so it kept nothing." >&2
    exit 4
fi
chgrp -- "$group" "$file"
chmod 0640 -- "$file"
"#;

const REMOVE: &str = r#"
set -eu
dir="$1/.horizon-drops/$2/$3"
if [ -L "$dir" ] || [ ! -d "$dir" ]; then exit 0; fi
rm -rf -- "$dir"
"#;

/// The worker and session that a drop goes to, captured when the person drops the files.
#[derive(Clone, Debug)]
pub struct Destination {
    worker: Worker,
    state_root: PathBuf,
    cloud_id: String,
    session: String,
}

impl Destination {
    /// Reads nothing, so the UI thread can call it.
    ///
    /// # Errors
    /// Refuses identities that the worker refuses.
    pub fn new(worker: &Worker, state_root: &Path, cloud_id: &str, session: &str) -> Result<Self> {
        if !valid_id(session) || !valid_id(&worker.id) {
            return Err(Error::Invalid("Invalid cloud session identity"));
        }
        cloud_directory(state_root, cloud_id)?;
        Ok(Self {
            worker: worker.clone(),
            state_root: state_root.to_path_buf(),
            cloud_id: cloud_id.to_owned(),
            session: session.to_owned(),
        })
    }

    #[must_use]
    pub fn worker_id(&self) -> &str {
        &self.worker.id
    }

    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The SSH connection to the bound worker, which accepts only the host key recorded
    /// for that worker. It reads the cloud settings, so call it on a worker thread.
    ///
    /// # Errors
    /// Fails when the settings cannot be read or the worker has no SSH endpoint.
    pub fn connect(&self) -> Result<Connection> {
        let settings = Settings::load(&self.state_root.join("settings.json"))?;
        Connection::new(
            &self.worker,
            &settings,
            &cloud_directory(&self.state_root, &self.cloud_id)?,
        )
    }

    /// A new folder name for one drop.
    #[must_use]
    pub fn batch(&self) -> Batch {
        Batch::new(WORKSPACE, &self.session)
    }
}

/// The worker folder of one drop and the commands that fill or remove it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    workspace: String,
    session: String,
    id: String,
}

impl Batch {
    fn new(workspace: &str, session: &str) -> Self {
        let mut id = uuid::Uuid::new_v4().simple().to_string();
        id.truncate(BATCH_DIGITS);
        Self {
            workspace: workspace.to_owned(),
            session: session.to_owned(),
            id,
        }
    }

    #[must_use]
    pub fn directory(&self) -> String {
        format!("{}/.horizon-drops/{}/{}", self.workspace, self.session, self.id)
    }

    /// The worker path of `name`, one of [`remote_names`].
    #[must_use]
    pub fn path(&self, name: &str) -> String {
        format!("{}/{name}", self.directory())
    }

    /// Makes the empty batch folder; fails when it exists.
    #[must_use]
    pub fn prepare_command(&self) -> String {
        command(PREPARE, &[&self.workspace, &self.session, &self.id])
    }

    /// Writes standard input to `name` in the batch folder and checks that it has `size` bytes.
    #[must_use]
    pub fn write_command(&self, name: &str, size: u64) -> String {
        command(
            WRITE,
            &[&self.workspace, &self.session, &self.id, name, &size.to_string()],
        )
    }

    /// Removes the batch folder and everything in it.
    #[must_use]
    pub fn remove_command(&self) -> String {
        command(REMOVE, &[&self.workspace, &self.session, &self.id])
    }
}

/// Worker file names for `paths`, in the same order and all different. Each is the local
/// name with control characters replaced by `_` and bytes that are not UTF-8 by `U+FFFD`,
/// shortened to the worker's limit. A repeated name gets ` (2)`, ` (3)` and so on before
/// its extension.
#[must_use]
pub fn remote_names(paths: &[PathBuf]) -> Vec<String> {
    let mut taken = HashSet::new();
    paths
        .iter()
        .map(|path| {
            let (stem, extension) = name_parts(path);
            let mut name = fit(&stem, &extension, "");
            let mut copy = 2_u32;
            while !taken.insert(name.clone()) {
                name = fit(&stem, &extension, &format!(" ({copy})"));
                copy += 1;
            }
            name
        })
        .collect()
}

fn name_parts(path: &Path) -> (String, String) {
    let clean = |text: &std::ffi::OsStr| -> String {
        text.to_string_lossy()
            .chars()
            .map(|character| if character.is_control() { '_' } else { character })
            .collect()
    };
    let name = path.file_name().map(clean).unwrap_or_default();
    if name.is_empty() || name == "." || name == ".." {
        return ("file".into(), String::new());
    }
    let stem = Path::new(&name).file_stem().map(clean).unwrap_or_default();
    let extension = Path::new(&name)
        .extension()
        .map(|extension| format!(".{}", clean(extension)))
        .unwrap_or_default();
    (stem, extension)
}

/// `stem`, `suffix` and `extension` joined, with the stem shortened at a character boundary
/// so that the name fits in [`NAME_MAX`] bytes. An extension too long to keep counts as stem.
fn fit(stem: &str, extension: &str, suffix: &str) -> String {
    if extension.len() + suffix.len() >= NAME_MAX {
        return fit(&format!("{stem}{extension}"), "", suffix);
    }
    let room = NAME_MAX - extension.len() - suffix.len();
    let mut end = stem.len().min(room);
    while !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{suffix}{extension}", &stem[..end])
}

fn command(script: &str, args: &[&str]) -> String {
    let mut command = format!("sh -c {} --", quote(script.trim()));
    for arg in args {
        command.push(' ');
        command.push_str(&quote(arg));
    }
    command
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests;
