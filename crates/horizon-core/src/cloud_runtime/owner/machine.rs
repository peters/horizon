//! Native host identity; never derived from transferable cloud state or environment.
use super::{Error, Result};

#[derive(Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(super) struct MachineId(uuid::Uuid);

impl MachineId {
    pub(super) fn read() -> Result<Self> {
        native().and_then(|value| Self::parse(&value))
    }

    fn parse(value: &str) -> Result<Self> {
        let id = uuid::Uuid::parse_str(value.trim()).map_err(|_| Error::Machine)?;
        if id.is_nil() {
            return Err(Error::Machine);
        }
        Ok(Self(id))
    }
}

#[cfg(target_os = "linux")]
fn native() -> Result<String> {
    std::fs::read_to_string("/etc/machine-id").map_err(|_| Error::Machine)
}

#[cfg(target_os = "macos")]
fn native() -> Result<String> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .map_err(|_| Error::Machine)?;
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return Err(Error::Machine);
    }
    parse_ioreg(std::str::from_utf8(&output.stdout).map_err(|_| Error::Machine)?)
}

#[cfg(any(test, target_os = "macos"))]
fn parse_ioreg(output: &str) -> Result<String> {
    let mut values = output.lines().filter_map(|line| {
        let (key, value) = line.trim().split_once(" = ")?;
        (key == "\"IOPlatformUUID\"").then(|| value.trim_matches('"').to_owned())
    });
    let value = values.next().ok_or(Error::Machine)?;
    if values.next().is_some() {
        return Err(Error::Machine);
    }
    Ok(value)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn native() -> Result<String> {
    // The allocation journal also requires Unix directory durability.
    Err(Error::Machine)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_invalid_and_nil_machine_ids_never_establish_ownership() {
        for value in ["", "host-name", "00000000000000000000000000000000"] {
            assert!(MachineId::parse(value).is_err());
        }
        let raw = "123456789abcdef0123456789abcdef0\n";
        let canonical = "12345678-9abc-def0-1234-56789abcdef0";
        assert!(MachineId::parse(raw).unwrap() == MachineId::parse(canonical).unwrap());
    }

    #[test]
    fn platform_output_requires_exact_unambiguous_native_property() {
        let id = "12345678-9ABC-DEF0-1234-56789ABCDEF0";
        let line = format!("    \"IOPlatformUUID\" = \"{id}\"\n");
        assert_eq!(parse_ioreg(&line).unwrap(), id);
        assert!(parse_ioreg(&line.repeat(2)).is_err());
        assert!(parse_ioreg("\"model\" = \"example\"").is_err());
    }
}
