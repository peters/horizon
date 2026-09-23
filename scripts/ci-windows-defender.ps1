# Keep Windows Defender from scanning the compiler's output.
# Object files land in the workspace, Cargo home, and rustup. The runner's
# shared temporary directories stay scanned so later steps are not exempt.
# Exclusion failures are non-fatal: some hosted images deny the preference.
$ErrorActionPreference = 'Continue'
$paths = @(
    $env:GITHUB_WORKSPACE,
    (Join-Path $env:USERPROFILE '.cargo'),
    (Join-Path $env:USERPROFILE '.rustup')
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }
try {
    Add-MpPreference -ExclusionPath $paths -ErrorAction Stop
    Add-MpPreference -ExclusionProcess @(
        'cargo.exe', 'rustc.exe', 'rustdoc.exe', 'link.exe', 'rust-lld.exe'
    ) -ErrorAction Stop
    Write-Output 'defender exclusions added'
} catch {
    Write-Warning $_.Exception.Message
    Write-Output "defender exclusion skipped: $($_.Exception.Message)"
}
exit 0
