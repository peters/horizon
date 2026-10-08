//! Complete first publication and recoverable updates of Horizon-owned skill trees.
use std::{collections::BTreeMap, io, path::Path};

use super::{EmbeddedFile, mcp_skills::validate_skill_files, sync_file_if_changed, sync_plugin_files};

const RECORD: &str = ".horizon-owned.json";
const MAX_RECORD_BYTES: u64 = 256 * 1024;

struct Ownership {
    version: u32,
    files: BTreeMap<String, String>,
    pending: Option<BTreeMap<String, String>>,
}

fn contents(files: &[EmbeddedFile]) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|file| (file.relative_path.into(), file.content.into()))
        .collect()
}

fn refusal() -> io::Error {
    io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Skill has user changes; preserving existing content",
    )
}

fn record(dir: &Path) -> io::Result<Option<Ownership>> {
    let path = dir.join(RECORD);
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
        Ok(metadata) if metadata.is_file() && metadata.len() <= MAX_RECORD_BYTES => {
            let json: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join(RECORD))?).map_err(io::Error::other)?;
            let object = json.as_object().ok_or_else(refusal)?;
            if object.len() != 3
                || object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "version" | "files" | "pending"))
            {
                return Err(refusal());
            }
            let value = Ownership {
                version: json["version"]
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(refusal)?,
                files: serde_json::from_value(json["files"].clone()).map_err(io::Error::other)?,
                pending: serde_json::from_value(json["pending"].clone()).map_err(io::Error::other)?,
            };
            if value.version != 1 || value.files.is_empty() {
                return Err(refusal());
            }
            for name in value.files.keys().chain(value.pending.iter().flat_map(BTreeMap::keys)) {
                if name == RECORD
                    || Path::new(name)
                        .components()
                        .any(|part| !matches!(part, std::path::Component::Normal(_)))
                {
                    return Err(refusal());
                }
            }
            Ok(Some(value))
        }
        Ok(_) => Err(refusal()),
    }
}

pub(super) fn validate(dir: &Path, files: &[EmbeddedFile]) -> io::Result<()> {
    match std::fs::symlink_metadata(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(refusal()),
    }
    let Some(owned) = record(dir)? else {
        return validate_skill_files(dir, files);
    };
    let mut seen = Vec::new();
    examine(dir, dir, &owned, &mut seen)?;
    if owned
        .files
        .keys()
        .any(|name| !seen.contains(name) && owned.pending.as_ref().is_none_or(|next| next.contains_key(name)))
    {
        return Err(refusal());
    }
    Ok(())
}

fn examine(root: &Path, dir: &Path, owned: &Ownership, seen: &mut Vec<String>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(io::Error::other)?;
        if relative == Path::new(RECORD) {
            continue;
        }
        let old = owned.files.keys();
        let next = owned.pending.iter().flat_map(BTreeMap::keys);
        let kind = entry.file_type()?;
        if kind.is_dir() && old.chain(next).any(|name| Path::new(name).starts_with(relative)) {
            examine(root, &path, owned, seen)?;
        } else if kind.is_file() {
            let name = relative.to_str().ok_or_else(refusal)?.replace('\\', "/");
            let value = std::fs::read_to_string(&path)?;
            if owned.files.get(&name) != Some(&value)
                && owned.pending.as_ref().and_then(|files| files.get(&name)) != Some(&value)
            {
                return Err(refusal());
            }
            seen.push(name);
        } else {
            return Err(refusal());
        }
    }
    Ok(())
}

fn observed_contents(
    dir: &Path,
    owned: &Ownership,
    desired: &BTreeMap<String, String>,
) -> io::Result<BTreeMap<String, String>> {
    let mut observed = BTreeMap::new();
    for name in owned.files.keys().chain(owned.pending.iter().flat_map(BTreeMap::keys)) {
        match std::fs::read_to_string(dir.join(name)) {
            Ok(value) => {
                observed.insert(name.clone(), value);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !desired.contains_key(name)
                    && let Some(value) = owned
                        .files
                        .get(name)
                        .or_else(|| owned.pending.as_ref().and_then(|files| files.get(name)))
                {
                    observed.insert(name.clone(), value.clone());
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(observed)
}

fn write_record(dir: &Path, owned: &Ownership) -> io::Result<bool> {
    let value =
        serde_json::json!({"version": owned.version, "files": owned.files, "pending": owned.pending}).to_string();
    if value.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Skill ownership record exceeds its limit",
        ));
    }
    sync_file_if_changed(&dir.join(RECORD), &value)
}

pub(super) fn sync(dir: &Path, files: &[EmbeddedFile]) -> io::Result<usize> {
    super::user_skills::with_skill_root_lock(dir, || {
        validate(dir, files)?;
        let desired = contents(files);
        if !dir.try_exists()? {
            let parent = dir.parent().ok_or_else(refusal)?;
            let staged = tempfile::Builder::new().prefix(".horizon-skill-").tempdir_in(parent)?;
            sync_plugin_files(staged.path(), files)?;
            write_record(
                staged.path(),
                &Ownership {
                    version: 1,
                    files: desired,
                    pending: None,
                },
            )?;
            std::fs::rename(staged.path(), dir)?;
            return Ok(files.len() + 1);
        }
        let prior = record(dir)?;
        let previous = match prior.as_ref() {
            Some(owned) => observed_contents(dir, owned, &desired)?,
            None => desired.clone(),
        };
        if prior
            .as_ref()
            .is_some_and(|owned| owned.files == desired && owned.pending.is_none())
        {
            return Ok(0);
        }
        // The record names both generations before any file changes. A restart
        // accepts only those exact bytes and can finish an interrupted update.
        let transition = Ownership {
            version: 1,
            files: previous.clone(),
            pending: Some(desired.clone()),
        };
        write_record(dir, &transition)?;
        let mut updated = sync_plugin_files(dir, files)?;
        for name in previous.keys().filter(|name| !desired.contains_key(*name)) {
            let path = dir.join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let mut parent = path.parent();
            while let Some(candidate) = parent.filter(|candidate| *candidate != dir) {
                match std::fs::remove_dir(candidate) {
                    Ok(()) => parent = candidate.parent(),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => parent = candidate.parent(),
                    Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
                    Err(error) => return Err(error),
                }
            }
            updated += 1;
        }
        updated += usize::from(write_record(
            dir,
            &Ownership {
                version: 1,
                files: desired,
                pending: None,
            },
        )?);
        Ok(updated)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_interruption_before_added_reference_preserves_recovery() {
        let temp = tempfile::tempdir().expect("temp");
        let dir = temp.path().join("horizon-cast");
        let old = [EmbeddedFile {
            relative_path: "SKILL.md",
            content: "old entry point",
        }];
        let new = [
            EmbeddedFile {
                relative_path: "SKILL.md",
                content: "new entry point",
            },
            EmbeddedFile {
                relative_path: "references/new.md",
                content: "new reference",
            },
        ];
        sync(&dir, &old).expect("old installation");
        let desired = contents(&new);
        let first = Ownership {
            version: 1,
            files: contents(&old),
            pending: Some(desired.clone()),
        };
        write_record(&dir, &first).expect("first interrupted transition");
        validate(&dir, &new).expect("first interruption remains recoverable");
        let second = Ownership {
            version: 1,
            files: observed_contents(&dir, &first, &desired).expect("observed baseline"),
            pending: Some(desired),
        };
        write_record(&dir, &second).expect("second interrupted transition");
        validate(&dir, &new).expect("second interruption remains recoverable");
        sync(&dir, &new).expect("finish publication");
        validate(&dir, &new).expect("complete new generation");
        assert_eq!(
            std::fs::read_to_string(dir.join("references/new.md")).expect("new reference"),
            "new reference"
        );
    }
}
