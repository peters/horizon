---
applyTo: "examples/cloud-worker/Dockerfile,examples/cloud-worker/Dockerfile.helpers,examples/cloud-worker/tailscale/**,examples/cloud-worker/prepare-context.py,examples/cloud-worker/test_prepare_context.py,examples/cloud-worker/.dockerignore"
---

Read docs/architecture/tailscale-unix-proxy.md. Review the upstream patch as
executable production/test source. Preserve archive/compiler pins, module
checksums, consumed notices and Go-only build stages. Both native architectures
must run complete ./ipn, ./ipn/ipnlocal and ./cmd/tailscale/cli package tests.
Marker equality is not binary reproducibility; older non-descriptor passes do
not qualify the new descriptor/half-close/ACK contract.

--horizon-flow-cancel / TCPPortHandler.HorizonFlowCancel defaults false and is
Serve-only: Linux raw TCP, absolute filesystem Unix, PROXY1, nonzero virtual port,
no TLS/HTTP/Funnel. Validate proposed CLI and direct SetServeConfig updates before
mutation, recursively including Foreground/Services, and validate restored
handlers. Reject matching-port Funnel across scopes/API and at actual ingress.
Rejected updates retain config; generated clone/view and status agree.
False/absent retains ordinary TCP v1/v2, Unix v0/v1 and generic proxy behavior.

Exactly first header byte P carries one SCM_RIGHTS cancellation endpoint. No
payload framing or ambiguous send retry. Sender owns the sent-end until send
returns, then closes rather than shutdowns it; cancellation interrupts original
backend, never races a raw descriptor close. No hidden sent-end duplicate.
Real payload CloseWrite preserves reverse copy/control custody, including gonet
and metered frontend wrappers. Consumer control EOF/data/error or daemon shutdown
cancels blocked header/response writes and joins all started goroutines/FDs.

Only two successful copies plus both required CloseWrite operations permit one
control DRAINED=0x01 followed by EOF. No ACK on error or ambiguous ACK retry.
Serialize cancellation with terminal success, retain first meaningful failure,
and join sender/copies/control reader on every exit. ACK proves daemon copying
and frontend half-close, not remote application delivery. Whole cancellation
must remain effective while a consumer finish operation retains a private owner.

This runtime grants no source authority or M2 activation. Future consumers must
reject missing/extra/truncated/late/wrong FDs, validate same original root daemon
and private listener, and keep bounded parser/generation/destination identity
checks. Consumer normal completion requires exact ACK+EOF under existing flow
and grace deadlines; EOF alone is failure. Require separate genuine Kernel and
userspace Worker cases. No same-PID TCP/generic-proxy substitute, arbitrary UDS
route, metadata authority, invented published pin or transferred old TCP proof.
