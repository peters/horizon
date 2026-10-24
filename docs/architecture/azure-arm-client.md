# Azure ARM client

The module `horizon_cloud::azure` is the Azure Resource Manager (ARM) client for
CPU workers. It sends bounded requests to ARM, classifies each ARM answer, reads
resource groups and VMs into typed views and reconciles an operation by name. No
Horizon UI, CLI or MCP operation calls it in this change.
`offers::AZURE_DEPLOYABLE` stays `false`.

## Trust boundary

The caller supplies one `Credential` (a bearer token), one subscription id and the
names of the resources. ARM supplies untrusted HTTP answers: status, headers and
body. A body and an error message are untrusted data.

The client guarantees these items:

- Each URL starts with `https://management.azure.com/subscriptions/<subscription>/`.
  The client builds it only from validated segments and an explicit API version.
  A bad segment gives `CloudError::Invalid` and sends nothing.
- The production agent accepts HTTPS only, follows no redirect and stops each
  request after 30 seconds.
- A success body of 4 MiB or more gives `InvalidResponse`. The client reads a
  maximum of 8 KiB of a failure body and keeps only a sanitized `Reason`. A
  reason that repeats the credential is removed.
- A 2xx answer whose `id`, `name` or `vmId` does not fit the request gives
  `IdentityMismatch`. The client never returns a partial view of it.
- A 409 is never retried and never a definite refusal. A caller must reconcile.
- The client sends GET requests only.
- `reconcile` adopts a group only when the group name is
  `horizon-ws-<operation id>` and its `horizon-operation` tag is that operation
  id. It never tags, changes or deletes a group that it finds.

The client does not guarantee these items:

- It does not prove that the credential belongs to the subscription. ARM decides
  access.
- It does not keep a durable record. A later pull request adds the creation
  journal and the state machine on `reconcile`.
- It does not prove that a `Created` VM runs the expected image. `vmId` names the
  instance only.
- A `loopback` client sends HTTP to a loopback address. Only tests use it.
