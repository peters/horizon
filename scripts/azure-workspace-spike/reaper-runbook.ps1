param([Parameter(Mandatory = $true)][string]$SubscriptionId)
$ErrorActionPreference = 'Stop'
if ($SubscriptionId -notmatch '^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$') { throw "SubscriptionId must be a UUID" }
Disable-AzContextAutosave -Scope Process | Out-Null
Connect-AzAccount -Identity -Subscription $SubscriptionId | Out-Null
$context = Set-AzContext -Subscription $SubscriptionId
if ($context.Subscription.Id -ne $SubscriptionId) { throw "Az context is $($context.Subscription.Id), expected $SubscriptionId" }
$now = (Get-Date).ToUniversalTime()
$acted = 0
foreach ($vm in Get-AzVM -Status) {
  if ($null -eq $vm.Tags -or $vm.Tags['purpose'] -ne 'horizon-azure-vm-spike' -or -not $vm.Tags['deadline']) { continue }
  try { $deadline = [datetime]::Parse($vm.Tags['deadline'], $null, [System.Globalization.DateTimeStyles]::AdjustToUniversal) } catch { Write-Output "skip $($vm.Name): unparsable deadline"; continue }
  if ($now -le $deadline) { continue }
  if ($vm.PowerState -eq 'VM deallocated' -or $vm.PowerState -eq 'VM deallocating') { continue }
  Write-Output "deallocating $($vm.ResourceGroupName)/$($vm.Name) (deadline $($deadline.ToString('u')), now $($now.ToString('u')))"
  try { Stop-AzVM -ResourceGroupName $vm.ResourceGroupName -Name $vm.Name -Force -NoWait | Out-Null; $acted++ }
  catch { Write-Output "failed to deallocate $($vm.ResourceGroupName)/$($vm.Name): $($_.Exception.Message)" }
}
Write-Output "reaper done: $acted deallocation request(s)"
