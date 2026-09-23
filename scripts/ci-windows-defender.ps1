# Keep Windows Defender from scanning compiler inputs and object files.
# Object files land in the workspace target directory. Cargo home holds
# registry and git sources, and rustup holds the toolchain. The runner's
# shared temporary directories stay scanned so later steps are not exempt.
# Process-name exclusions are omitted: they would also cover an untrusted
# cargo.exe or link.exe placed inside the already-excluded workspace.
# Exclusion failures are non-fatal: some hosted images deny the preference.
$ErrorActionPreference = 'Continue'
$paths = @(
    $env:GITHUB_WORKSPACE,
    (Join-Path $env:USERPROFILE '.cargo'),
    (Join-Path $env:USERPROFILE '.rustup')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
try {
    if ($paths) {
        Add-MpPreference -ExclusionPath $paths -ErrorAction Stop
    }
    Write-Output 'defender exclusions added'
} catch {
    Write-Warning $_.Exception.Message
    Write-Output "defender exclusion skipped: $($_.Exception.Message)"
}
exit 0
