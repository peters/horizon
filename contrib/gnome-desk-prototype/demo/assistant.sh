#!/bin/bash
# What the stand-in assistant does after it hears the request.
D=$(cd "$(dirname "$0")" && pwd); C="python3 $D/../bin/mcp_call.py"
$C plan @$D/p0.json >/dev/null
sleep 1.5
$C send api-agent "Fix the failing clone_cleanup test in the api crate, then run cargo test." >/dev/null; echo "  → api-agent (Api service): fix the failing test"
sleep 2.5
$C send site-agent "Update the pricing page copy: three plans and a 20 percent annual discount." >/dev/null; echo "  → site-agent (Marketing site): update the pricing copy"
sleep 2.5
$C send infra-agent "Check the idle cloud environment hetzner-cx42 and prepare to stop it." >/dev/null; echo "  → infra-agent (Cloud): check the idle environment"
$C plan @$D/p1.json >/dev/null
sleep 9
$C plan @$D/p2.json >/dev/null
sleep 5
$C plan @$D/p3.json >/dev/null
sleep 5
$C plan @$D/p4.json >/dev/null
$C note "Recap" "**2 of 3 done, 1 needs you**|- **API:** clone_cleanup fixed, the suite is green|- **Site:** pricing copy updated|- **Cloud:** hetzner-cx42 is idle. Approve stopping it?" >/dev/null
echo "● Recap posted. The cloud agent needs your answer."
