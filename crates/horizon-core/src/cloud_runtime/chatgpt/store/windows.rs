//! User-scoped Windows ACLs without widening the core crate's unsafe-code policy.
use super::{Error, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use std::{
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

// The path is an environment value, never PowerShell source. Only the current
// Windows SID may own or have access to a credential object. A fresh descriptor
// drops both inherited and explicit access for other principals.
const ACL: &str = r#"
$ErrorActionPreference = 'Stop'
$path = $env:HORIZON_CREDENTIAL_PATH
$sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User
$item = Get-Item -LiteralPath $path -Force
if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'reparse point' }
$acl = Get-Acl -LiteralPath $path
if ($env:HORIZON_CREDENTIAL_PROTECT -eq '1') {
    $owner = $acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value
    # Elevated Windows processes can create objects owned by the Administrators group.
    # SetOwner must still succeed and the resulting descriptor must name this user.
    if ($owner -ne $sid.Value -and $owner -ne 'S-1-5-32-544') { throw 'foreign owner' }
    if ($item.PSIsContainer) {
        $acl = New-Object System.Security.AccessControl.DirectorySecurity
        $inheritance = [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
    } else {
        $acl = New-Object System.Security.AccessControl.FileSecurity
        $inheritance = [System.Security.AccessControl.InheritanceFlags]::None
    }
    $acl.SetOwner($sid)
    $acl.SetAccessRuleProtection($true, $false)
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($sid, 'FullControl', $inheritance, 'None', 'Allow')
    $acl.AddAccessRule($rule)
    Set-Acl -LiteralPath $path -AclObject $acl
    $acl = Get-Acl -LiteralPath $path
}
if (-not $acl.AreAccessRulesProtected) { throw 'inherited access' }
if ($acl.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -ne $sid.Value) { throw 'foreign owner' }
$rules = @($acl.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))
if ($rules.Count -ne 1) { throw 'unexpected access rules' }
$rule = $rules[0]
if ($rule.IsInherited -or $rule.IdentityReference.Value -ne $sid.Value -or $rule.AccessControlType -ne 'Allow' -or $rule.FileSystemRights -ne 'FullControl') { throw 'not user scoped' }
"#;

pub(super) fn protect(path: &Path) -> Result<()> {
    run(path, ACL, true)
}

pub(super) fn verify(path: &Path) -> Result<()> {
    run(path, ACL, false)
}

fn run(path: &Path, script: &str, protect: bool) -> Result<()> {
    use std::os::windows::process::CommandExt as _;
    let system_root =
        std::env::var_os("SystemRoot").ok_or(Error::Invalid("Windows system directory is unavailable"))?;
    let executable = Path::new(&system_root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut child = Command::new(executable)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(STANDARD.encode(bytes))
        .env("HORIZON_CREDENTIAL_PATH", path)
        .env("HORIZON_CREDENTIAL_PROTECT", if protect { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "credential access must be restricted to this Windows user",
                ))
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Invalid("Windows credential permission verification timed out"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publicly_readable_records_and_directories_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("chatgpt");
        std::fs::create_dir(&directory).unwrap();
        protect(&directory).unwrap();
        let path = directory.join("record.json");
        std::fs::write(&path, b"synthetic").unwrap();
        protect(&path).unwrap();
        assert!(verify(&directory).is_ok());
        assert!(verify(&path).is_ok());
        const MAKE_PUBLIC: &str = r#"
$ErrorActionPreference = 'Stop'
$acl = Get-Acl -LiteralPath $env:HORIZON_CREDENTIAL_PATH
$everyone = New-Object System.Security.Principal.SecurityIdentifier('S-1-1-0')
$rule = New-Object System.Security.AccessControl.FileSystemAccessRule($everyone, 'Read', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $env:HORIZON_CREDENTIAL_PATH -AclObject $acl
"#;
        run(&path, MAKE_PUBLIC, false).unwrap();
        assert!(verify(&path).is_err());
        assert!(super::super::read_private(&path).is_err());
        protect(&path).unwrap();
        run(&directory, MAKE_PUBLIC, false).unwrap();
        assert!(verify(&directory).is_err());
        assert!(super::super::read_private(&path).is_err());
    }
}
