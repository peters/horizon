#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Run the full Surge filesystem installer/update smoke on an Azure Windows 11 VM.

This provisions a Windows VM, installs MSVC build tools if needed, stages the
local-filesystem smoke runner, forces one autologon to create a real desktop
session, launches the smoke through an interactive scheduled task, and then
polls the guest-side logs until the run succeeds or fails.

If `--resource-group` and `--vm-name` point at an existing VM, the script reuses
that machine instead of provisioning a new one. Reused VMs are always kept.

Usage:
  run-surge-azure-smoke.sh \
    [--repo-url <https-url>] \
    [--branch <git-branch>] \
    [--commit-sha <git-sha>] \
    [--location <azure-region>] \
    [--image <publisher:offer:sku:version>] \
    [--size <vm-size>] \
    [--resource-group <name>] \
    [--vm-name <name>] \
    [--admin-username <name>] \
    [--admin-password <password>] \
    [--poll-interval-seconds <seconds>] \
    [--session-timeout-seconds <seconds>] \
    [--smoke-timeout-seconds <seconds>] \
    [--keep-resources]
EOF
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'Required command not found: %s\n' "$1" >&2
    exit 1
  fi
}

normalize_repo_url() {
  local raw_url="$1"

  case "$raw_url" in
    git@github.com:*)
      raw_url="${raw_url#git@github.com:}"
      printf 'https://github.com/%s\n' "$raw_url"
      ;;
    ssh://git@github.com/*)
      raw_url="${raw_url#ssh://git@github.com/}"
      printf 'https://github.com/%s\n' "$raw_url"
      ;;
    https://*|http://*)
      printf '%s\n' "$raw_url"
      ;;
    *)
      printf 'Unsupported repo URL for guest clone: %s\n' "$raw_url" >&2
      exit 1
      ;;
  esac
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

azure_group_exists() {
  [ "$(az group exists --name "$resource_group" -o tsv 2>/dev/null || true)" = "true" ]
}

azure_vm_exists() {
  az vm show \
    --resource-group "$resource_group" \
    --name "$vm_name" \
    --query id \
    -o tsv >/dev/null 2>&1
}

vm_power_state() {
  az vm get-instance-view \
    --resource-group "$resource_group" \
    --name "$vm_name" \
    --query "instanceView.statuses[?starts_with(code, 'PowerState/')].displayStatus | [0]" \
    -o tsv 2>/dev/null || true
}

wait_for_vm_agent_ready() {
  local attempts="${1:-40}"
  local sleep_seconds="${2:-15}"

  for _ in $(seq 1 "$attempts"); do
    vm_agent_status="$(az vm get-instance-view \
      --resource-group "$resource_group" \
      --name "$vm_name" \
      --query "instanceView.vmAgent.statuses[].displayStatus" \
      -o tsv 2>/dev/null | tail -n 1 || true)"
    if [ "$vm_agent_status" = "Ready" ]; then
      return 0
    fi
    sleep "$sleep_seconds"
  done

  return 1
}

wait_for_public_ip() {
  local attempts="${1:-40}"
  local sleep_seconds="${2:-15}"

  for _ in $(seq 1 "$attempts"); do
    public_ip="$(az vm show \
      --resource-group "$resource_group" \
      --name "$vm_name" \
      -d \
      --query publicIps \
      -o tsv 2>/dev/null || true)"
    if [ -n "$public_ip" ] && [ "$public_ip" != "null" ]; then
      return 0
    fi
    sleep "$sleep_seconds"
  done

  return 1
}

create_buildtools_script() {
  local buildtools_path="$1"

  cat >"$buildtools_path" <<'EOF'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$smokeRoot = "C:\horizon-surge-smoke"
$installerPath = Join-Path $smokeRoot "vs_BuildTools.exe"
$vsDevCmd = "C:\BuildTools\Common7\Tools\VsDevCmd.bat"

if (Test-Path $vsDevCmd) {
    Write-Output "VS_BUILDTOOLS=present"
    exit 0
}

New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null
Invoke-WebRequest -Uri "https://aka.ms/vs/17/release/vs_BuildTools.exe" -OutFile $installerPath
& $installerPath `
    --quiet `
    --wait `
    --norestart `
    --nocache `
    --installPath C:\BuildTools `
    --add Microsoft.VisualStudio.Workload.VCTools `
    --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
    --add Microsoft.VisualStudio.Component.Windows11SDK.22621

if ($LASTEXITCODE -ne 0) {
    throw "vs_BuildTools.exe failed with exit code $LASTEXITCODE"
}

if (-not (Test-Path $vsDevCmd)) {
    throw "Build Tools install reported success but $vsDevCmd is still missing."
}

Write-Output "VS_BUILDTOOLS=installed"
EOF
}

create_stage_script() {
  local stage_path="$1"

  cat >"$stage_path" <<'EOF'
param(
    [Parameter(Mandatory = $true)]
    [string]$AdminUsername,

    [Parameter(Mandatory = $true)]
    [string]$AdminPassword,

    [Parameter(Mandatory = $true)]
    [string]$RepoUrl,

    [Parameter(Mandatory = $true)]
    [string]$Branch,

    [Parameter(Mandatory = $true)]
    [string]$CommitSha
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

function Write-Utf8File {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path,

        [Parameter(Mandatory = $true)]
        [string]$Content
    )

    $encoding = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $encoding)
}

$smokeRoot = "C:\horizon-surge-smoke"
$runnerPath = Join-Path $smokeRoot "run-smoke.ps1"
$cmdPath = Join-Path $smokeRoot "run-smoke-inner.cmd"
$statePath = Join-Path $smokeRoot "state.txt"
$errorPath = Join-Path $smokeRoot "error.txt"
$streamPath = Join-Path $smokeRoot "smoke.stream.log"
$transcriptPath = Join-Path $smokeRoot "smoke.log"
$sessionPath = Join-Path $smokeRoot "session.txt"
$gitExe = "C:\Program Files\Git\cmd\git.exe"
$gitBash = "C:\Program Files\Git\bin\bash.exe"
$vsDevCmd = "C:\BuildTools\Common7\Tools\VsDevCmd.bat"

if (-not (Test-Path $gitExe)) {
    throw "Git executable not found at $gitExe"
}

if (-not (Test-Path $gitBash)) {
    throw "Git Bash not found at $gitBash"
}

if (-not (Test-Path $vsDevCmd)) {
    throw "VsDevCmd.bat not found at $vsDevCmd"
}

New-Item -ItemType Directory -Force -Path $smokeRoot | Out-Null
Remove-Item -LiteralPath $runnerPath, $cmdPath, $errorPath, $streamPath, $transcriptPath, $sessionPath -Force -ErrorAction SilentlyContinue

$innerCmd = @"
@echo off
call "$vsDevCmd" -arch=amd64 -host_arch=amd64
if errorlevel 1 exit /b 1
set "PATH=%USERPROFILE%\.cargo\bin;C:\Program Files\Git\bin;%PATH%"
"$gitBash" -lc "cd /c/horizon-surge-smoke/repo && ./scripts/run-surge-filesystem-smoke.sh --rid win-x64"
exit /b %ERRORLEVEL%
"@
Write-Utf8File -Path $cmdPath -Content $innerCmd

$runnerTemplate = @'
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$smokeRoot = 'C:\horizon-surge-smoke'
$repoDir = Join-Path $smokeRoot 'repo'
$statePath = Join-Path $smokeRoot 'state.txt'
$errorPath = Join-Path $smokeRoot 'error.txt'
$streamPath = Join-Path $smokeRoot 'smoke.stream.log'
$transcriptPath = Join-Path $smokeRoot 'smoke.log'
$sessionPath = Join-Path $smokeRoot 'session.txt'
$rustupInitPath = Join-Path $smokeRoot 'rustup-init.exe'
$gitExe = 'C:\Program Files\Git\cmd\git.exe'
$repoUrl = '__REPO_URL__'
$branch = '__BRANCH__'
$commitSha = '__COMMIT_SHA__'
$rustupUrl = 'https://win.rustup.rs/x86_64'
$cmdPath = Join-Path $smokeRoot 'run-smoke-inner.cmd'

function Invoke-CmdLogged {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Command
    )

    $cmdLogPath = Join-Path $env:TEMP ("horizon-surge-native-{0}.log" -f ([guid]::NewGuid().ToString("N")))
    & cmd.exe /d /c "$Command > `"$cmdLogPath`" 2>&1"
    $exitCode = $LASTEXITCODE

    if (Test-Path $cmdLogPath) {
        Get-Content -LiteralPath $cmdLogPath | Tee-Object -FilePath $streamPath -Append | Out-Default
        Remove-Item -LiteralPath $cmdLogPath -Force -ErrorAction SilentlyContinue
    }

    return $exitCode
}

Remove-Item -LiteralPath $errorPath, $streamPath -Force -ErrorAction SilentlyContinue
Set-Content -LiteralPath $statePath -Value 'running'

Start-Transcript -Path $transcriptPath -Force | Out-Null
try {
    $sessionId = (Get-Process -Id $PID).SessionId
    $sessionSummary = "user=$env:USERNAME interactive=$([Environment]::UserInteractive) session_id=$sessionId"
    Set-Content -LiteralPath $sessionPath -Value $sessionSummary
    Write-Output "SESSION_INFO $sessionSummary"

    $cargoExe = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path $cargoExe)) {
        Write-Output 'INSTALL Rust toolchain'
        Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupInitPath
        & $rustupInitPath -y --default-toolchain stable --profile minimal --default-host x86_64-pc-windows-msvc
        if ($LASTEXITCODE -ne 0) {
            throw "rustup-init failed with exit code $LASTEXITCODE"
        }
    }

    if (-not (Test-Path $repoDir)) {
        $gitCloneExit = Invoke-CmdLogged "`"$gitExe`" clone --branch `"$branch`" --single-branch `"$repoUrl`" `"$repoDir`""
        if ($gitCloneExit -ne 0) {
            throw "git clone failed with exit code $gitCloneExit"
        }
    }

    Push-Location $repoDir
    try {
        $gitRemoteExit = Invoke-CmdLogged "`"$gitExe`" remote set-url origin `"$repoUrl`""
        if ($gitRemoteExit -ne 0) {
            throw "git remote set-url failed with exit code $gitRemoteExit"
        }
        $gitFetchExit = Invoke-CmdLogged "`"$gitExe`" fetch origin `"$branch`""
        if ($gitFetchExit -ne 0) {
            throw "git fetch failed with exit code $gitFetchExit"
        }
        $gitCheckoutExit = Invoke-CmdLogged "`"$gitExe`" checkout --force `"$branch`""
        if ($gitCheckoutExit -ne 0) {
            throw "git checkout failed with exit code $gitCheckoutExit"
        }
        $gitResetExit = Invoke-CmdLogged "`"$gitExe`" reset --hard `"$commitSha`""
        if ($gitResetExit -ne 0) {
            throw "git reset failed with exit code $gitResetExit"
        }
        $gitLfsInstallExit = Invoke-CmdLogged "`"$gitExe`" lfs install --local"
        if ($gitLfsInstallExit -ne 0) {
            throw "git lfs install failed with exit code $gitLfsInstallExit"
        }
        $gitLfsPullExit = Invoke-CmdLogged "`"$gitExe`" lfs pull"
        if ($gitLfsPullExit -ne 0) {
            throw "git lfs pull failed with exit code $gitLfsPullExit"
        }
    }
    finally {
        Pop-Location
    }

    $smokeExit = Invoke-CmdLogged "`"$cmdPath`""
    if ($smokeExit -ne 0) {
        throw "smoke command failed with exit code $smokeExit"
    }

    Set-Content -LiteralPath $statePath -Value 'succeeded'
}
catch {
    ($_ | Out-String).Trim() | Set-Content -LiteralPath $errorPath
    Set-Content -LiteralPath $statePath -Value 'failed'
    throw
}
finally {
    Stop-Transcript | Out-Null
}
'@

$runner = $runnerTemplate.
    Replace('__REPO_URL__', $RepoUrl).
    Replace('__BRANCH__', $Branch).
    Replace('__COMMIT_SHA__', $CommitSha)
Write-Utf8File -Path $runnerPath -Content $runner

$winlogon = 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon'
Set-ItemProperty -Path $winlogon -Name AutoAdminLogon -Value '1'
Set-ItemProperty -Path $winlogon -Name DefaultUserName -Value $AdminUsername
Set-ItemProperty -Path $winlogon -Name DefaultPassword -Value $AdminPassword
Set-ItemProperty -Path $winlogon -Name DefaultDomainName -Value $env:COMPUTERNAME
Set-ItemProperty -Path $winlogon -Name ForceAutoLogon -Value '1'

Set-Content -LiteralPath $statePath -Value 'prepared'
Write-Output "STAGED"
Write-Output "SMOKE_ROOT=$smokeRoot"
Write-Output "RUNNER=$runnerPath"
Write-Output "AUTOLOGON_USER=$AdminUsername"
EOF
}

create_start_task_script() {
  local start_path="$1"

  cat >"$start_path" <<'EOF'
param(
    [Parameter(Mandatory = $true)]
    [string]$AdminUsername
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$taskName = "HorizonSurgeSmoke"
$runnerPath = "C:\horizon-surge-smoke\run-smoke.ps1"
$userId = "$env:COMPUTERNAME\$AdminUsername"

Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue

$action = New-ScheduledTaskAction -Execute "powershell.exe" -Argument "-NoLogo -NoProfile -ExecutionPolicy Bypass -File `"$runnerPath`""
$principal = New-ScheduledTaskPrincipal -UserId $userId -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -ExecutionTimeLimit (New-TimeSpan -Hours 12)
$task = New-ScheduledTask -Action $action -Principal $principal -Settings $settings

Register-ScheduledTask -TaskName $taskName -InputObject $task -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
Start-Sleep -Seconds 5

$scheduledTask = Get-ScheduledTask -TaskName $taskName
$taskInfo = Get-ScheduledTaskInfo -TaskName $taskName
Write-Output "TASK_STATE=$($scheduledTask.State)"
Write-Output "TASK_RESULT=$($taskInfo.LastTaskResult)"
EOF
}

create_poll_script() {
  local poll_path="$1"

  cat >"$poll_path" <<'EOF'
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$smokeRoot = "C:\horizon-surge-smoke"
$statePath = Join-Path $smokeRoot "state.txt"
$sessionPath = Join-Path $smokeRoot "session.txt"
$errorPath = Join-Path $smokeRoot "error.txt"
$streamPath = Join-Path $smokeRoot "smoke.stream.log"
$taskName = "HorizonSurgeSmoke"

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

$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($task) {
    $taskInfo = Get-ScheduledTaskInfo -TaskName $taskName
    Write-Output "TASK_STATE=$($task.State)"
    Write-Output "TASK_RESULT=$($taskInfo.LastTaskResult)"
}

if (Test-Path $streamPath) {
    Get-Content -LiteralPath $streamPath | Select-Object -Last 30 | ForEach-Object {
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

$smokeRoot = "C:\horizon-surge-smoke"
$taskName = "HorizonSurgeSmoke"
$paths = @(
    (Join-Path $smokeRoot "state.txt"),
    (Join-Path $smokeRoot "session.txt"),
    (Join-Path $smokeRoot "error.txt"),
    (Join-Path $smokeRoot "smoke.stream.log"),
    (Join-Path $smokeRoot "smoke.log"),
    (Join-Path $smokeRoot "run-smoke.ps1"),
    (Join-Path $smokeRoot "run-smoke-inner.cmd")
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

Write-Output "===== scheduled-task ====="
$task = Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
if ($task) {
    $task | Select-Object TaskName, State | Format-List | Out-String -Width 200 | Write-Output
    Get-ScheduledTaskInfo -TaskName $taskName | Select-Object LastRunTime, LastTaskResult, NumberOfMissedRuns | Format-List | Out-String -Width 200 | Write-Output
}
else {
    Write-Output "<missing>"
}
EOF
}

repo_url=""
branch=""
commit_sha=""
location="northeurope"
image="MicrosoftVisualStudio:windowsplustools:base-win11-gen2:latest"
size="Standard_D4s_v3"
resource_group=""
vm_name=""
computer_name=""
admin_username="azureuser"
admin_password="CodexSurge!${RANDOM}${RANDOM}${RANDOM}Aa1"
poll_interval_seconds=30
session_timeout_seconds=60
smoke_timeout_seconds=7200
keep_resources=false

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo-url)
      repo_url="${2:-}"
      shift 2
      ;;
    --branch)
      branch="${2:-}"
      shift 2
      ;;
    --commit-sha)
      commit_sha="${2:-}"
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
    --session-timeout-seconds)
      session_timeout_seconds="${2:-}"
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

require_command az
require_command git

if [ -z "$repo_url" ]; then
  repo_url="$(git remote get-url origin)"
fi
repo_url="$(normalize_repo_url "$repo_url")"

if [ -z "$branch" ]; then
  branch="$(git rev-parse --abbrev-ref HEAD)"
fi

if [ -z "$commit_sha" ]; then
  commit_sha="$(git rev-parse HEAD)"
fi

if [ -z "$resource_group" ]; then
  resource_group="horizon-surge-win-smoke-$(date -u +%Y%m%d%H%M%S)-rg"
fi

if [ -z "$vm_name" ]; then
  vm_name="hzwin$(date -u +%m%d%H%M%S)"
fi

computer_name="$(printf '%s' "$vm_name" | tr -cd '[:alnum:]-' | cut -c 1-15)"
if [ -z "$computer_name" ] || printf '%s\n' "$computer_name" | grep -Eq '^[0-9]+$'; then
  computer_name="hzwin$(date -u +%m%d%H%M%S)"
fi

tmp_dir="$(mktemp -d)"
resource_group_created=false
reused_existing_vm=false
public_ip=""
vm_agent_status=""
final_logs_path="$(mktemp "/tmp/horizon-surge-azure-smoke-logs.XXXXXX.txt")"

cleanup() {
  rm -rf "$tmp_dir"

  if [ "$keep_resources" = true ] || [ "$reused_existing_vm" = true ]; then
    printf 'Keeping Azure resource group %s\n' "$resource_group" >&2
    printf 'Keeping Azure VM %s\n' "$vm_name" >&2
    printf 'Guest login: %s@%s\n' "$admin_username" "$public_ip" >&2
    return
  fi

  if [ "$resource_group_created" = true ]; then
    az group delete --name "$resource_group" --yes --no-wait >/dev/null
    printf 'Started deleting Azure resource group %s\n' "$resource_group" >&2
  fi
}
trap cleanup EXIT

buildtools_script="${tmp_dir}/buildtools.ps1"
stage_script="${tmp_dir}/stage.ps1"
start_task_script="${tmp_dir}/start-task.ps1"
poll_script="${tmp_dir}/poll.ps1"
fetch_logs_script="${tmp_dir}/fetch-logs.ps1"

create_buildtools_script "$buildtools_script"
create_stage_script "$stage_script"
create_start_task_script "$start_task_script"
create_poll_script "$poll_script"
create_fetch_logs_script "$fetch_logs_script"

printf 'Using repo %s\n' "$repo_url"
printf 'Using branch %s at %s\n' "$branch" "$commit_sha"

if azure_vm_exists; then
  reused_existing_vm=true
  current_power_state="$(vm_power_state)"
  printf 'Reusing existing Windows 11 VM %s (%s)\n' "$vm_name" "${current_power_state:-unknown state}"
  if [ "$current_power_state" != "VM running" ]; then
    printf 'Starting existing Windows 11 VM %s\n' "$vm_name"
    az vm start --resource-group "$resource_group" --name "$vm_name" -o none
  fi
else
  if ! azure_group_exists; then
    printf 'Creating Azure resource group %s in %s\n' "$resource_group" "$location"
    az group create --name "$resource_group" --location "$location" -o none
    resource_group_created=true
  else
    printf 'Using existing Azure resource group %s\n' "$resource_group"
  fi

  printf 'Creating Windows 11 VM %s (%s, %s)\n' "$vm_name" "$image" "$size"
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
fi

printf 'Waiting for the VM public IP address\n'
if ! wait_for_public_ip 40 15; then
  printf 'Failed to resolve the VM public IP address.\n' >&2
  exit 1
fi
printf 'VM public IP: %s\n' "$public_ip"

printf 'Waiting for Azure VM agent readiness\n'
if ! wait_for_vm_agent_ready 40 15; then
  printf 'Azure VM agent never reached Ready for %s.\n' "$vm_name" >&2
  exit 1
fi

printf 'Ensuring Visual Studio Build Tools are present\n'
printf '%s\n' "$(azure_run_powershell "$buildtools_script")"

printf 'Staging the interactive Surge smoke runner\n'
azure_run_powershell "$stage_script" \
  --parameters \
  "AdminUsername=${admin_username}" \
  "AdminPassword=${admin_password}" \
  "RepoUrl=${repo_url}" \
  "Branch=${branch}" \
  "CommitSha=${commit_sha}" \
  >/dev/null

if [ "$reused_existing_vm" = true ] && [ "${current_power_state:-}" = "VM running" ]; then
  printf 'Warm VM already running; skipping reboot and starting the interactive task directly\n'
else
  printf 'Restarting the VM to trigger autologon\n'
  az vm restart --resource-group "$resource_group" --name "$vm_name" -o none

  printf 'Waiting for Azure VM agent readiness after restart\n'
  if ! wait_for_vm_agent_ready 40 15; then
    printf 'Azure VM agent never reached Ready after restart for %s.\n' "$vm_name" >&2
    exit 1
  fi
  printf 'Waiting %s seconds for autologon to settle\n' "$session_timeout_seconds"
  sleep "$session_timeout_seconds"
fi

printf 'Starting the interactive scheduled-task smoke runner\n'
start_output="$(azure_run_powershell "$start_task_script" --parameters "AdminUsername=${admin_username}")"
printf '%s\n' "$start_output"
initial_task_state="$(printf '%s\n' "$start_output" | sed -n 's/^TASK_STATE=//p' | tail -n 1)"
initial_task_result="$(printf '%s\n' "$start_output" | sed -n 's/^TASK_RESULT=//p' | tail -n 1)"

if [ "$initial_task_state" = "Ready" ] && [ "$initial_task_result" = "267011" ]; then
  printf 'Interactive task has not run yet; restarting once to force autologon and retry\n'
  az vm restart --resource-group "$resource_group" --name "$vm_name" -o none

  printf 'Waiting for Azure VM agent readiness after fallback restart\n'
  if ! wait_for_vm_agent_ready 40 15; then
    printf 'Azure VM agent never reached Ready after fallback restart for %s.\n' "$vm_name" >&2
    exit 1
  fi

  printf 'Waiting %s seconds for autologon to settle after fallback restart\n' "$session_timeout_seconds"
  sleep "$session_timeout_seconds"

  printf 'Retrying the interactive scheduled-task smoke runner\n'
  start_output="$(azure_run_powershell "$start_task_script" --parameters "AdminUsername=${admin_username}")"
  printf '%s\n' "$start_output"
fi

deadline=$((SECONDS + smoke_timeout_seconds))
last_state=""
last_task_state=""
last_log_tail=""

while [ "$SECONDS" -lt "$deadline" ]; do
  poll_output="$(azure_run_powershell "$poll_script" 2>/dev/null || true)"
  current_state="$(printf '%s\n' "$poll_output" | sed -n 's/^STATE=//p' | tail -n 1)"
  current_task_state="$(printf '%s\n' "$poll_output" | sed -n 's/^TASK_STATE=//p' | tail -n 1)"

  if [ -z "$current_state" ]; then
    current_state="pending"
  fi

  if [ "$current_state" != "$last_state" ]; then
    printf 'Smoke state: %s\n' "$current_state"
    last_state="$current_state"
  fi

  if [ -n "$current_task_state" ] && [ "$current_task_state" != "$last_task_state" ]; then
    printf 'Scheduled task state: %s\n' "$current_task_state"
    last_task_state="$current_task_state"
  fi

  current_log_tail="$(printf '%s\n' "$poll_output" | sed -n 's/^LOG=//p')"
  if [ -n "$current_log_tail" ] && [ "$current_log_tail" != "$last_log_tail" ]; then
    printf '%s\n' "$current_log_tail"
    last_log_tail="$current_log_tail"
  fi

  if [ "$current_state" = "succeeded" ] || [ "$current_state" = "failed" ]; then
    break
  fi

  sleep "$poll_interval_seconds"
done

azure_run_powershell "$fetch_logs_script" >"$final_logs_path" || true
printf 'Full smoke logs saved to %s\n' "$final_logs_path"

if [ "$last_state" != "succeeded" ]; then
  printf 'Interactive Surge smoke did not succeed. Final logs:\n' >&2
  cat "$final_logs_path" >&2
  exit 1
fi

printf 'Interactive Surge smoke succeeded.\n'
