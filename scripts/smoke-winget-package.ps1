[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InstallManifestDir,

    [string]$UpgradeManifestDir,

    [string]$PackageIdentifier = "Peters.Horizon",

    [string]$ExpectedInstallVersion,

    [string]$ExpectedUpgradeVersion
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Resolve-WingetCommand {
    $command = Get-Command winget -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    $desktopAppInstaller = Get-AppxPackage -AllUsers Microsoft.DesktopAppInstaller |
        Select-Object -First 1

    if ($desktopAppInstaller -and $desktopAppInstaller.InstallLocation) {
        $candidate = Join-Path $desktopAppInstaller.InstallLocation "winget.exe"
        if (Test-Path $candidate) {
            return $candidate
        }
    }

    throw "winget.exe was not found on PATH or in the Microsoft.DesktopAppInstaller package."
}

$script:WingetCommand = Resolve-WingetCommand

function Enable-LocalManifestSupport {
    & $script:WingetCommand settings --enable LocalManifestFiles
    if ($LASTEXITCODE -ne 0) {
        throw "Failed to enable LocalManifestFiles in winget settings."
    }
}

function Invoke-Winget {
    param(
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    $extraArguments = @("--accept-source-agreements", "--disable-interactivity")
    if ($Arguments[0] -in @("install", "upgrade")) {
        $extraArguments += "--accept-package-agreements"
    }

    $output = & $script:WingetCommand @Arguments @extraArguments 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0) {
        throw "winget failed with exit code ${LASTEXITCODE}: winget $($Arguments -join ' ')`n$output"
    }

    return $output.Trim()
}

function Get-HorizonBinaryPath {
    $searchRoots = @(
        (Join-Path $env:ProgramFiles "WinGet\Packages"),
        (Join-Path $env:LOCALAPPDATA "Microsoft\WinGet\Packages")
    ) | Where-Object { $_ -and (Test-Path $_) }

    foreach ($root in $searchRoots) {
        $candidate = Get-ChildItem $root -Filter "horizon.exe" -Recurse -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTimeUtc -Descending |
            Select-Object -First 1

        if ($candidate) {
            return $candidate.FullName
        }
    }

    return $null
}

function Get-ExpectedInstallerHash {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ManifestDir
    )

    $installerManifestPath = Join-Path $ManifestDir "Peters.Horizon.installer.yaml"
    $content = Get-Content -Raw $installerManifestPath
    $match = [regex]::Match($content, "InstallerSha256:\s*([A-Fa-f0-9]{64})")
    if (-not $match.Success) {
        throw "Could not read InstallerSha256 from $installerManifestPath"
    }

    return $match.Groups[1].Value.ToUpperInvariant()
}

function Assert-HorizonBinaryMatchesManifest {
    param(
        [Parameter(Mandatory = $true)]
        [string]$ManifestDir
    )

    $binaryPath = Get-HorizonBinaryPath
    if (-not $binaryPath) {
        throw "Expected Horizon to be installed, but no horizon.exe was found under WinGet package roots."
    }

    $actualHash = (Get-FileHash -Path $binaryPath -Algorithm SHA256).Hash.ToUpperInvariant()
    $expectedHash = Get-ExpectedInstallerHash -ManifestDir $ManifestDir

    if ($actualHash -ne $expectedHash) {
        throw "Expected horizon.exe hash $expectedHash but found $actualHash at $binaryPath"
    }

    return $binaryPath
}

function Assert-HorizonMissing {
    $binaryPath = Get-HorizonBinaryPath
    if ($binaryPath) {
        throw "Expected Horizon to be absent after uninstall, but found $binaryPath"
    }
}

function Assert-HorizonLaunches {
    $binaryPath = Get-HorizonBinaryPath
    if (-not $binaryPath) {
        throw "Expected Horizon to be installed before launch validation."
    }

    $runningProcesses = Get-Process -Name "horizon" -ErrorAction SilentlyContinue
    if ($runningProcesses) {
        $runningProcesses | Stop-Process -Force
    }

    Start-Process -FilePath $binaryPath -WindowStyle Hidden

    Start-Sleep -Seconds 5

    $process = Get-Process -Name "horizon" -ErrorAction SilentlyContinue |
        Sort-Object StartTime -Descending |
        Select-Object -First 1

    if (-not $process) {
        throw "Installed Horizon process was not observed after launch."
    }

    $process | Stop-Process -Force
    Start-Sleep -Seconds 2

    if (Get-Process -Id $process.Id -ErrorAction SilentlyContinue) {
        throw "Installed Horizon process remained running after taskkill."
    }
}

Write-Host "INSTALL_START"
Enable-LocalManifestSupport

Invoke-Winget -Arguments @("install", "--manifest", $InstallManifestDir, "--scope", "machine", "--silent")
Write-Host "INSTALL_OK"

Assert-HorizonBinaryMatchesManifest -ManifestDir $InstallManifestDir | Out-Null
Write-Host "INSTALL_HASH_OK"

Write-Host "INSTALL_LAUNCH_START"
Assert-HorizonLaunches
Write-Host "INSTALL_LAUNCH_OK"

if ($UpgradeManifestDir) {
    Write-Host "UPGRADE_START"
    Invoke-Winget -Arguments @("upgrade", "--manifest", $UpgradeManifestDir, "--scope", "machine", "--silent")
    Write-Host "UPGRADE_OK"

    Assert-HorizonBinaryMatchesManifest -ManifestDir $UpgradeManifestDir | Out-Null
    Write-Host "UPGRADE_HASH_OK"

    Write-Host "UPGRADE_LAUNCH_START"
    Assert-HorizonLaunches
    Write-Host "UPGRADE_LAUNCH_OK"
}

$uninstallManifestDir = $InstallManifestDir
if ($UpgradeManifestDir) {
    $uninstallManifestDir = $UpgradeManifestDir
}

Write-Host "UNINSTALL_START"
Invoke-Winget -Arguments @("uninstall", "--manifest", $uninstallManifestDir, "--scope", "machine", "--silent", "--purge")
Assert-HorizonMissing
Write-Host "UNINSTALL_OK"
