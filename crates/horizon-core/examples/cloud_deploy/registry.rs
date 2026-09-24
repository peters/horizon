use horizon_core::cloud_runtime::{self, Cancellation, Error, registry, settings::Settings, setup::Draft};
use std::path::Path;

pub(super) fn run(args: &[String]) -> cloud_runtime::Result<()> {
    if args.len() != 3 {
        return Err(Error::Invalid(
            "Usage: cloud_deploy registry SETTINGS ACTION_JSON_FILE | registry-bind SETTINGS BINDING_JSON_FILE",
        ));
    }
    let settings_path = Path::new(&args[1]);
    let input = std::fs::read(&args[2])?;
    if args[0] == "registry" {
        let settings = Settings::load(settings_path)?;
        let action: registry::Action = serde_json::from_slice(&input).map_err(|_| Error::Json)?;
        let status = registry::manage(&settings, &action, &Cancellation::default())?;
        println!("{}", serde_json::to_string(&status).map_err(|_| Error::Json)?);
    } else if args[0] == "registry-bind" {
        if settings_path.file_name().and_then(|name| name.to_str()) != Some("settings.json") {
            return Err(Error::Invalid(
                "Registry setup requires the machine-local settings.json path",
            ));
        }
        let mut value: serde_json::Value = serde_json::from_slice(&input).map_err(|_| Error::Json)?;
        let object = value.as_object_mut().ok_or(Error::Json)?;
        object.insert("generation".into(), serde_json::Value::String(cloud_runtime::new_id()));
        let binding: registry::Binding = serde_json::from_value(value).map_err(|_| Error::Json)?;
        let mut draft = Draft::load(
            settings_path
                .parent()
                .ok_or(Error::Invalid("Settings directory is missing"))?,
        )?;
        let mut edit = registry::draft::Draft::from_binding(&binding);
        edit.pull_secret = private_secret(&binding.pull.secret_file)?;
        if let Some(auth) = &binding.publish {
            edit.publish_secret = private_secret(&auth.secret_file)?;
        }
        let index = draft
            .registries
            .iter()
            .position(|item| item.repository == binding.repository);
        edit.original = index.and_then(|index| draft.registries[index].original.clone());
        if let Some(index) = index {
            draft.registries[index] = edit;
        } else {
            draft.registries.push(edit);
        }
        draft.save()?;
        println!("Registry binding saved privately. Validate an immutable image before use.");
    } else {
        return Err(Error::Invalid("Unknown registry command"));
    }
    Ok(())
}

fn private_secret(path: &Path) -> cloud_runtime::Result<zeroize::Zeroizing<String>> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid("Registry secret file must be private (0600)"));
        }
    }
    if !path.is_absolute() || !meta.is_file() || meta.len() == 0 || meta.len() > 4096 {
        return Err(Error::Invalid(
            "Use an absolute path to a nonempty private registry secret file",
        ));
    }
    let value = zeroize::Zeroizing::new(std::fs::read_to_string(path)?);
    if value.trim().is_empty() {
        return Err(Error::Invalid(
            "Registry credential file must contain a nonempty credential",
        ));
    }
    Ok(zeroize::Zeroizing::new(value.trim().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_whitespace_secret_cannot_mean_keep_saved_credential() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential");
        std::fs::write(&path, " \t\r\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(private_secret(&path).is_err());
        std::fs::write(&path, "synthetic-token\n").unwrap();
        assert_eq!(private_secret(&path).unwrap().as_str(), "synthetic-token");
    }
}
