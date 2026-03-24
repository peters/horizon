#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run the full WinGet smoke test on an Azure Windows 11 VM inside a live RDP desktop session.

Usage:
  run-winget-azure-smoke.sh \
    --install-version <version> \
    --install-sha <sha256> \
    [--install-release-date <YYYY-MM-DD>] \
    [--upgrade-version <version>] \
    [--upgrade-sha <sha256>] \
    [--upgrade-release-date <YYYY-MM-DD>] \
    [--location <azure-region>] \
    [--image <publisher:offer:sku:version>] \
    [--size <vm-size>] \
    [--resource-group <name>] \
    [--vm-name <name>] \
    [--admin-username <name>] \
    [--admin-password <password>] \
    [--poll-interval-seconds <seconds>] \
    [--smoke-timeout-seconds <seconds>] \
    [--keep-resources]
EOF
}

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd -- "${script_dir}/.." && pwd)"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "$1" >&2
    exit 1
  fi
}

azure_run_powershell() {
  local script_path="$1"
  shift
  az vm run-command invoke \
    --resource-group "$resource_group" \
    --name "$vm_name" \
    --command-id RunPowerShellScript \
    --scripts @"$script_path" \
    "$@" \
    --query "value[0].message" \
    -o tsv
}

create_stage_script() {
  local stage_path="$1"
  local render_script_b64
  local smoke_script_b64
  render_script_b64="$(base64 < "${repo_root}/scripts/render-winget-manifests.ps1" | tr -d '\n')"
  smoke_script_b64="$(base64 < "${repo_root}/scripts/smoke-winget-package.ps1" | tr -d '\n')"

  cat >"$stage_path" <<EOF
param(
    [Parameter(Mandatory = \$true)]
    [string]\$AdminUsername,

    [Parameter(Mandatory = \$true)]
    [string]\$InstallVersion,

    [Parameter(Mandatory = \$true)]
    [string]\$InstallSha,

    [Parameter(Mandatory = \$true)]
    [string]\$InstallReleaseDate,

    [string]\$UpgradeVersion,

    [string]\$UpgradeSha,

    [string]\$UpgradeReleaseDate
)

Set-StrictMode -Version Latest
\$ErrorActionPreference = "Stop"
\$ProgressPreference = "SilentlyContinue"

function Write-Utf8File {
    param(
        [Parameter(Mandatory = \$true)]
        [string]\$Path,

        [Parameter(Mandatory = \$true)]
        [string]\$Content
    )

    \$encoding = New-Object System.Text.UTF8Encoding(\$false)
    [System.IO.File]::WriteAllText(\$Path, \$Content, \$encoding)
}

function Write-Base64File {
    param(
        [Parameter(Mandatory = \$true)]
        [string]\$Path,

        [Parameter(Mandatory = \$true)]
        [string]\$Base64
    )

    [System.IO.File]::WriteAllBytes(\$Path, [System.Convert]::FromBase64String(\$Base64))
}

\$smokeRoot = "C:\\horizon-winget-smoke"
\$startupDir = "C:\\Users\\\$AdminUsername\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup"
\$renderScriptPath = Join-Path \$smokeRoot "render-winget-manifests.ps1"
\$smokeScriptPath = Join-Path \$smokeRoot "smoke-winget-package.ps1"
\$runnerPath = Join-Path \$smokeRoot "run-smoke.ps1"
\$launcherPath = Join-Path \$startupDir "horizon-winget-smoke.cmd"
\$statePath = Join-Path \$smokeRoot "state.txt"
\$errorPath = Join-Path \$smokeRoot "error.txt"
\$logPath = Join-Path \$smokeRoot "smoke.log"
\$streamPath = Join-Path \$smokeRoot "smoke.stream.log"
\$sessionPath = Join-Path \$smokeRoot "session.txt"
\$installManifestDir = Join-Path \$smokeRoot "install-manifest"
\$upgradeManifestDir = Join-Path \$smokeRoot "upgrade-manifest"

New-Item -ItemType Directory -Force -Path \$smokeRoot, \$startupDir | Out-Null
Remove-Item -LiteralPath \$launcherPath, \$statePath, \$errorPath, \$logPath, \$streamPath, \$sessionPath -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath \$installManifestDir, \$upgradeManifestDir -Recurse -Force -ErrorAction SilentlyContinue

Write-Base64File -Path \$renderScriptPath -Base64 '${render_script_b64}'
Write-Base64File -Path \$smokeScriptPath -Base64 '${smoke_script_b64}'

& \$renderScriptPath -OutputDir \$installManifestDir -Version \$InstallVersion -InstallerSha256 \$InstallSha -ReleaseDate \$InstallReleaseDate

\$runnerUpgradeLine = '\$null'
if (\$UpgradeVersion) {
    & \$renderScriptPath -OutputDir \$upgradeManifestDir -Version \$UpgradeVersion -InstallerSha256 \$UpgradeSha -ReleaseDate \$UpgradeReleaseDate
    \$runnerUpgradeLine = "\$smokeArgs.UpgradeManifestDir = '\$upgradeManifestDir'"
}

\$runnerContent = @"
Set-StrictMode -Version Latest
\$ErrorActionPreference = 'Stop'
\$ProgressPreference = 'SilentlyContinue'

\$smokeRoot = 'C:\\horizon-winget-smoke'
\$launcherPath = 'C:\\Users\\\$AdminUsername\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Startup\\horizon-winget-smoke.cmd'
\$statePath = Join-Path \$smokeRoot 'state.txt'
\$errorPath = Join-Path \$smokeRoot 'error.txt'
\$logPath = Join-Path \$smokeRoot 'smoke.log'
\$streamPath = Join-Path \$smokeRoot 'smoke.stream.log'
\$sessionPath = Join-Path \$smokeRoot 'session.txt'
\$smokeScriptPath = Join-Path \$smokeRoot 'smoke-winget-package.ps1'
\$smokeArgs = @{
    InstallManifestDir = Join-Path \$smokeRoot 'install-manifest'
}
\${runnerUpgradeLine}

Remove-Item -LiteralPath \$launcherPath -Force -ErrorAction SilentlyContinue
Remove-Item -LiteralPath \$errorPath, \$streamPath -Force -ErrorAction SilentlyContinue
Set-Content -LiteralPath \$statePath -Value 'running'

Start-Transcript -Path \$logPath -Force | Out-Null
try {
    \$sessionId = (Get-Process -Id \$PID).SessionId
    \$sessionSummary = "user=\$env:USERNAME interactive=\$([Environment]::UserInteractive) session_id=\$sessionId"
    Set-Content -LiteralPath \$sessionPath -Value \$sessionSummary
    Write-Output "SESSION_INFO \$sessionSummary"
    & \$smokeScriptPath @smokeArgs 2>&1 | Tee-Object -FilePath \$streamPath -Append | Out-Default
    Set-Content -LiteralPath \$statePath -Value 'succeeded'
}
catch {
    (\$_ | Out-String).Trim() | Set-Content -LiteralPath \$errorPath
    Set-Content -LiteralPath \$statePath -Value 'failed'
    throw
}
finally {
    Stop-Transcript | Out-Null
}
"@

\$launcherContent = @"
@echo off
start "" powershell.exe -NoLogo -NoProfile -ExecutionPolicy Bypass -File "\$runnerPath"
"@

Write-Utf8File -Path \$runnerPath -Content \$runnerContent
Write-Utf8File -Path \$launcherPath -Content \$launcherContent
Set-Content -LiteralPath \$statePath -Value 'prepared'
Write-Output "STAGED"
Write-Output "SMOKE_ROOT=\$smokeRoot"
Write-Output "LAUNCHER=\$launcherPath"
EOF
}

create_poll_script() {
  local poll_path="$1"

  cat >"$poll_path" <<'EOF'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$smokeRoot = "C:\horizon-winget-smoke"
$statePath = Join-Path $smokeRoot "state.txt"
$sessionPath = Join-Path $smokeRoot "session.txt"
$errorPath = Join-Path $smokeRoot "error.txt"
$streamPath = Join-Path $smokeRoot "smoke.stream.log"

if (Test-Path $statePath) {
    $state = (Get-Content -LiteralPath $statePath -Raw).Trim()
}
else {
    $state = "pending"
}

Write-Output "STATE=$state"

if (Test-Path $sessionPath) {
    $session = (Get-Content -LiteralPath $sessionPath -Raw).Trim()
    if ($session) {
        Write-Output "SESSION=$session"
    }
}

if (Test-Path $errorPath) {
    $errorSummary = (Get-Content -LiteralPath $errorPath -Raw).Trim()
    if ($errorSummary) {
        Write-Output "ERROR=$errorSummary"
    }
}

if (Test-Path $streamPath) {
    Get-Content -LiteralPath $streamPath | Select-Object -Last 20 | ForEach-Object {
        Write-Output "LOG=$_"
    }
}
EOF
}

create_fetch_logs_script() {
  local fetch_path="$1"

  cat >"$fetch_path" <<'EOF'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$smokeRoot = "C:\horizon-winget-smoke"
$paths = @(
    (Join-Path $smokeRoot "state.txt"),
    (Join-Path $smokeRoot "session.txt"),
    (Join-Path $smokeRoot "error.txt"),
    (Join-Path $smokeRoot "smoke.stream.log"),
    (Join-Path $smokeRoot "smoke.log")
)

foreach ($path in $paths) {
    Write-Output "===== $path ====="
    if (Test-Path $path) {
        (Get-Content -LiteralPath $path -Raw).TrimEnd("`r", "`n")
    }
    else {
        Write-Output "<missing>"
    }
}
EOF
}

start_rdp_session() {
  local log_path="$1"
  local rdp_args=(
    "/cert:ignore"
    "/u:${admin_username}"
    "/p:${admin_password}"
    "/v:${public_ip}:3389"
    "/log-level:WARN"
  )

  if [ -n "${DISPLAY:-}" ]; then
    xfreerdp "${rdp_args[@]}" >"$log_path" 2>&1 &
  else
    xvfb-run -a xfreerdp "${rdp_args[@]}" >"$log_path" 2>&1 &
  fi
  rdp_pid=$!
}

install_version=""
install_sha=""
install_release_date="$(date -u +%F)"
upgrade_version=""
upgrade_sha=""
upgrade_release_date="$(date -u +%F)"
location="northeurope"
image="MicrosoftWindowsDesktop:windows-11:win11-24h2-pro:latest"
size="Standard_B2ms"
resource_group=""
vm_name=""
computer_name=""
admin_username="azureuser"
admin_password="CodexWinget!$(date -u +%s)"
poll_interval_seconds=15
smoke_timeout_seconds=900
keep_resources=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --install-version)
      install_version="${2:-}"
      shift 2
      ;;
    --install-sha)
      install_sha="${2:-}"
      shift 2
      ;;
    --install-release-date)
      install_release_date="${2:-}"
      shift 2
      ;;
    --upgrade-version)
      upgrade_version="${2:-}"
      shift 2
      ;;
    --upgrade-sha)
      upgrade_sha="${2:-}"
      shift 2
      ;;
    --upgrade-release-date)
      upgrade_release_date="${2:-}"
      shift 2
      ;;
    --location)
      location="${2:-}"
      shift 2
      ;;
    --image)
      image="${2:-}"
      shift 2
      ;;
    --size)
      size="${2:-}"
      shift 2
      ;;
    --resource-group)
      resource_group="${2:-}"
      shift 2
      ;;
    --vm-name)
      vm_name="${2:-}"
      shift 2
      ;;
    --admin-username)
      admin_username="${2:-}"
      shift 2
      ;;
    --admin-password)
      admin_password="${2:-}"
      shift 2
      ;;
    --poll-interval-seconds)
      poll_interval_seconds="${2:-}"
      shift 2
      ;;
    --smoke-timeout-seconds)
      smoke_timeout_seconds="${2:-}"
      shift 2
      ;;
    --keep-resources)
      keep_resources=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'Unknown argument: %s\n\n' "$1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [ -z "$install_version" ] || [ -z "$install_sha" ]; then
  printf 'The install version and install SHA256 are required.\n\n' >&2
  usage >&2
  exit 1
fi

if [ -n "$upgrade_version" ] && [ -z "$upgrade_sha" ]; then
  printf 'The upgrade SHA256 is required when --upgrade-version is set.\n' >&2
  exit 1
fi

if [ -z "$upgrade_version" ] && [ -n "$upgrade_sha" ]; then
  printf 'The upgrade version is required when --upgrade-sha is set.\n' >&2
  exit 1
fi

if [ -z "$resource_group" ]; then
  resource_group="horizon-winget-smoke-$(date -u +%Y%m%d%H%M%S)-rg"
fi

if [ -z "$vm_name" ]; then
  vm_name="hzw11smoke$(date -u +%m%d%H%M%S)"
fi

computer_name="$(printf '%s' "$vm_name" | tr -cd '[:alnum:]-' | cut -c 1-15)"
if [ -z "$computer_name" ] || printf '%s\n' "$computer_name" | grep -Eq '^[0-9]+$'; then
  computer_name="hzsm$(date -u +%m%d%H%M%S)"
fi

require_command az
require_command base64
require_command xfreerdp
if [ -z "${DISPLAY:-}" ]; then
  require_command xvfb-run
fi

tmp_dir="$(mktemp -d)"
rdp_pid=""
resource_group_created=false

cleanup() {
  if [ -n "$rdp_pid" ] && kill -0 "$rdp_pid" >/dev/null 2>&1; then
    kill "$rdp_pid" >/dev/null 2>&1 || true
    wait "$rdp_pid" 2>/dev/null || true
  fi

  rm -rf "$tmp_dir"

  if [ "$keep_resources" = true ]; then
    printf 'Keeping Azure resource group %s\n' "$resource_group" >&2
    return
  fi

  if [ "$resource_group_created" = true ]; then
    az group delete --name "$resource_group" --yes --no-wait >/dev/null
    printf 'Started deleting Azure resource group %s\n' "$resource_group" >&2
  fi
}
trap cleanup EXIT

stage_script="${tmp_dir}/stage.ps1"
poll_script="${tmp_dir}/poll.ps1"
fetch_logs_script="${tmp_dir}/fetch-logs.ps1"
rdp_log_path="${tmp_dir}/rdp.log"
final_logs_path="$(mktemp "/tmp/horizon-winget-smoke-logs.XXXXXX.txt")"

create_stage_script "$stage_script"
create_poll_script "$poll_script"
create_fetch_logs_script "$fetch_logs_script"

printf 'Creating Azure resource group %s in %s\n' "$resource_group" "$location"
az group create --name "$resource_group" --location "$location" -o none
resource_group_created=true

printf 'Creating Windows 11 VM %s\n' "$vm_name"
az vm create \
  --resource-group "$resource_group" \
  --name "$vm_name" \
  --computer-name "$computer_name" \
  --image "$image" \
  --size "$size" \
  --admin-username "$admin_username" \
  --admin-password "$admin_password" \
  --authentication-type password \
  --nsg-rule RDP \
  --no-wait \
  -o none

public_ip=""
printf 'Waiting for the VM public IP address\n'
for _ in $(seq 1 40); do
  public_ip="$(az vm show --resource-group "$resource_group" --name "$vm_name" -d --query publicIps -o tsv 2>/dev/null || true)"
  if [ -n "$public_ip" ] && [ "$public_ip" != "null" ]; then
    break
  fi
  sleep 15
done

if [ -z "$public_ip" ] || [ "$public_ip" = "null" ]; then
  printf 'Failed to resolve the VM public IP address.\n' >&2
  exit 1
fi

printf 'VM public IP: %s\n' "$public_ip"
printf 'Waiting for Azure VM agent readiness\n'
for _ in $(seq 1 40); do
  vm_agent_status="$(az vm get-instance-view --resource-group "$resource_group" --name "$vm_name" --query "instanceView.vmAgent.statuses[].displayStatus" -o tsv 2>/dev/null | tail -n 1 || true)"
  if [ "$vm_agent_status" = "Ready" ]; then
    break
  fi
  sleep 15
done

if [ "${vm_agent_status:-}" != "Ready" ]; then
  printf 'Azure VM agent never reached Ready for %s.\n' "$vm_name" >&2
  exit 1
fi

printf 'Staging interactive smoke files on the VM\n'
azure_run_powershell "$stage_script" \
  --parameters \
  "AdminUsername=${admin_username}" \
  "InstallVersion=${install_version}" \
  "InstallSha=${install_sha}" \
  "InstallReleaseDate=${install_release_date}" \
  "UpgradeVersion=${upgrade_version}" \
  "UpgradeSha=${upgrade_sha}" \
  "UpgradeReleaseDate=${upgrade_release_date}" \
  >/dev/null

printf 'Starting RDP session to trigger the Startup-launched PowerShell smoke\n'
start_rdp_session "$rdp_log_path"
sleep 20

deadline=$((SECONDS + smoke_timeout_seconds))
last_state=""
last_log_tail=""

while [ "$SECONDS" -lt "$deadline" ]; do
  poll_output="$(azure_run_powershell "$poll_script" 2>/dev/null || true)"
  current_state="$(printf '%s\n' "$poll_output" | sed -n 's/^STATE=//p' | tail -n 1)"
  if [ -z "$current_state" ]; then
    current_state="pending"
  fi

  if [ "$current_state" != "$last_state" ]; then
    printf 'Smoke state: %s\n' "$current_state"
    last_state="$current_state"
  fi

  current_log_tail="$(printf '%s\n' "$poll_output" | sed -n 's/^LOG=//p')"
  if [ -n "$current_log_tail" ] && [ "$current_log_tail" != "$last_log_tail" ]; then
    printf '%s\n' "$current_log_tail"
    last_log_tail="$current_log_tail"
  fi

  if [ "$current_state" = "succeeded" ] || [ "$current_state" = "failed" ]; then
    break
  fi

  if ! kill -0 "$rdp_pid" >/dev/null 2>&1; then
    printf 'RDP session exited before the smoke finished.\n' >&2
    break
  fi

  sleep "$poll_interval_seconds"
done

azure_run_powershell "$fetch_logs_script" >"$final_logs_path" || true
printf 'Full smoke logs saved to %s\n' "$final_logs_path"

if [ "$last_state" != "succeeded" ]; then
  printf 'Interactive WinGet smoke did not succeed. Final logs:\n' >&2
  cat "$final_logs_path" >&2
  exit 1
fi

printf 'Interactive WinGet smoke succeeded.\n'
