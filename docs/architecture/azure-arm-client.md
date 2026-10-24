# Azure ARM client

The module `horizon_cloud::azure` is the Azure Resource Manager (ARM) client for
CPU workers. It sends bounded requests to ARM, classifies each ARM answer, reads
resource groups, deployments and VMs into typed views, submits one template
deployment, polls an ARM operation and reconciles an operation by name. No
Horizon UI, CLI or MCP operation calls it in this change.
`offers::AZURE_DEPLOYABLE` stays `false`.

## Trust boundary

The caller supplies one `Credential` (a bearer token), one subscription id and the
names of the resources. ARM supplies untrusted HTTP answers: status, headers and
body. A body, an error message and an operation URL are untrusted data.

The client guarantees these items:

- Each URL starts with `https://management.azure.com/subscriptions/<subscription>/`.
  The client builds it only from validated segments and an explicit API version.
  A bad segment gives `CloudError::Invalid` and sends nothing.
- The production agent accepts HTTPS only, follows no redirect and stops each
  request after 30 seconds. A poll request gets only the time that is left of
  its 10-minute bound.
- A success body of 4 MiB or more gives `InvalidResponse`. The client reads a
  maximum of 8 KiB of a failure body and keeps only a sanitized `Reason`. A
  reason that repeats the credential is removed.
- The client polls an `Azure-AsyncOperation` URL exactly as ARM sent it, only
  after `owns_operation_url` proved that it is under the same subscription and
  endpoint. A URL that a parser can normalize to another path is refused.
- A 2xx answer whose `id`, `name` or `vmId` does not fit the request gives
  `IdentityMismatch`. The client never returns a partial view of it.
- A 409 is never retried and never a definite refusal. A caller must reconcile.
- The only write is the PUT of `put_deployment`. `reconcile` sends GET requests
  only. It adopts a group only when the group name is `horizon-ws-<operation id>`
  and its `horizon-operation` tag is that operation id. It never tags, changes or
  deletes a group that it finds.

The client does not guarantee these items:

- It does not prove that the credential belongs to the subscription. ARM decides
  access.
- It does not keep a durable record. A later pull request adds the creation
  journal and the state machine on `reconcile`.
- It does not prove that a `Created` VM runs the expected image. `vmId` names the
  instance only.
- A `loopback` client sends HTTP to a loopback address. Only tests use it.
