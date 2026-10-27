---
applyTo: "crates/horizon-cloud/src/browser_vm.rs,crates/horizon-cloud/src/browser_vm/**,crates/horizon-core/src/cloud_runtime/browser_vm.rs,crates/horizon-core/src/cloud_runtime/browser_vm/**,crates/horizon-browser-control/src/manifest/browser_vm.rs,crates/horizon-browser-mcp/src/controller/browser_vm.rs,crates/horizon-ui/src/app/browser_vm_requests.rs,crates/horizon-ui/src/app/settings/browser_vm/**"
---

# Trust boundary of the browser VM deploy

`horizon_cloud::browser_vm` rents one Hetzner server with a state volume and runs
the browser image of `packaging/browser-vm` on it.
`cloud_runtime::browser_vm` keeps its record, reads the credential of the machine,
writes and removes the ssh entry that reaches the machine, and carries stop,
resume and delete out. An agent asks for it with the
`browser_vm` MCP tool. A person asks for the same requests in the Browser VM
section of the Settings window. Both reach
`crates/horizon-ui/src/app/browser_vm_requests.rs` through the provider usage
queue. The host plan of that server is `crates/horizon-cloud/src/host.rs`, which
the worker hosts share.

## What is trusted

- The Hetzner account, its token file and the SSH identity that the cloud
  settings name. The settings file, the token file and the key pair are private
  files of the person on this computer.
- The image reference. A person publishes the image, and the machine pulls it.
  This code does not build an image.
- The Hetzner API answers. A server, a volume or a key that does not carry the
  operation label `browser-vm` and its recorded name is never changed or used.
- The ssh configuration of the person and the ssh tools of this computer. Horizon
  reads that file, writes one marked block at its top, and reads the host keys of
  the machine with `ssh-keyscan`. It trusts nothing else in that file and no other
  file that the configuration includes.

## What Horizon guarantees

- Every field of the tool call is checked before any request: the image must hold
  `@sha256:` and a valid reference, the server type must be a dedicated `ccx`
  type, the location must be one the Hetzner settings allow, and the state volume
  must be 10 to 10,240 GB. A refused call writes no record and sends no request.
- The Browser VM section takes the same fields from a person and sends them to
  the same checks. The section shows every refusal as text. The image reference
  and the SSH target of the machine are the only values that the section reads
  back, and the SSH target reaches the settings entry `browser_host` only when
  `ssh` takes it as one argument.
- The machine is rented through the same provider code as a cloud worker: the
  same volume fence, server fence and SSH key record, each saved durably before
  the request it guards. A lost answer is reconciled by label, never repeated.
- The state volume is named after the operation, is attached without automount,
  and its user data mounts it at `/state`. The container publishes the VNC port on
  the loopback of the host only, and it holds no worker environment.
- The machine accepts the person's own Ed25519 key for root and no password. The
  host keeps its SSH server, because the person signs in on the display through
  that route.
- The tool answers with the phase it recorded, and `status` reads the record
  without a provider request: a read never starts one. One provider run at a
  time: a second one is refused while the first is in flight, and a read is
  answered meanwhile. A phase that a crash left in progress is taken over by the
  next explicit deploy, stop or delete.
- A failed run keeps its record. Stop keeps the state volume. Delete removes the
  server, the volume and the key, each proven absent, and only a delete removes a
  sign-in.
- The answer holds the SSH target and the Device panel route. It holds no token,
  no key material and no provider session.
- Horizon writes one marked block into `~/.ssh/config`, at the top, so that the
  first matching value of each keyword is Horizon's even below a `Host *`:
  `Host horizon-browser-vm`, the address of the machine, `User root`, the
  `IdentityFile` that the cloud settings name, `IdentitiesOnly yes`, and a
  `UserKnownHostsFile` in the cloud root of this Horizon. The block is followed by
  a closing marker. The block holds no other keyword.
- Every byte outside the block is kept. The write goes through a temporary file in
  the same folder and a rename, so a reader sees the old file or the new one. The
  folder is set to 0700 and the file to 0600. The managed known_hosts file is
  0600 and holds the keys of this one machine, keyed by the alias.
- Horizon refuses to write or to remove anything when a line outside its block
  names `horizon-browser-vm`, in any case, because ssh compares a host name
  without case. The deploy still succeeds, `ssh_target` stays `root@<address>`,
  and `ssh_note` says that the name is taken.
- The host keys of the machine are scanned at the deploy and at the resume, with
  a bounded retry, and written into the managed known_hosts file of this Horizon.
  A scan that fails does not fail the run: the entry stays, and `ssh_note` records
  the reason. Nothing is ever written into `~/.ssh/known_hosts` of the person.
- A stop and a delete remove exactly the marked block and the managed known_hosts
  file. The hosts and comments of the person stay as they were.
- Every provider call has a time limit and stops when the run is cancelled.

## What Horizon does not guarantee

- The image reference is only checked for shape. Horizon does not read the image,
  and a private image needs the `registry_pull` binding of the cloud settings.
- The Browser VM section names no field of its own. A person who types another
  image reference rents another machine, and Horizon cannot tell who typed it.
- Horizon does not verify what the machine runs after it starts. A person can
  change the container on that host with the person's own SSH key.
- The display of the machine is protected by the SSH route only. The VNC server
  of the image has no password, and a program on the machine can reach the
  loopback port.
- The browser VM is one machine for the whole Horizon. Two callers, an agent or a
  person in the Settings window, that rent, stop or delete at the same time are
  serialized by the record of this computer, not across two Horizon windows that
  share the file.
- A person with SSH access to the machine can read the profiles and the sign-ins
  of the state volume.
- Horizon changes no other ssh file and no other host block. It does not touch
  `~/.ssh/known_hosts`, `~/.ssh/authorized_keys`, the identity the cloud settings
  name, or the system `/etc/ssh` files. It does not copy a public key of the
  person into the machine and does not widen the tool contracts.
- Horizon reads only `~/.ssh/config` itself, not a file that an `Include` line of
  that file names. A host block for `horizon-browser-vm` inside an included file
  is invisible to the refusal above, and ssh may still resolve it first.
- A person who edits the block, or who names the alias in their own block after a
  deploy, can point `horizon-browser-vm` at another machine. Horizon then refuses
  to change that file again, and the answer says so.
- The host keys are trusted because the scan reached the address that the provider
  returned for this machine. Horizon does not compare them with a key that a
  person read out of band.
