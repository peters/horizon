# Protected host receipt reader

## Scope and trust boundary

The Linux Worker `protected-ingress` feature is off by default.
It adds an in-process reader for `/run/horizon-host-ingress/receipt.json`.
The reader accepts no path, request JSON or requested allocation.
The allocation comes from the genuine fixed read-only host receipt after root and
kernel custody checks. The original host producer verifies the Docker allocation
label, container, image, init start and network namespace before publishing it.
The reader retains that allocation with the original bytes. Neither a startup
capture nor a legacy environment value substitutes for this host boundary.
A future controller must compare its original owned allocation before effects.
It adds no command, listener, Core operation, Gate activation or provider effect.

Receipt bytes and serialized facts are untrusted comparison data.
The reader uses the canonical Protocol `Receipt::from_json` parser and its 16 KiB limit.
The parser checks identities and the complete claimed publication set.
It does not establish a current kernel boundary or source preservation.
See the [passive contract](exposure-protected-ingress.md).

## Original file custody

The reader opens each fixed directory component with `openat`, directory checks and no symlink following.
The root and `/run` must belong to UID 0 without group or other write permission.
The mounted generation directory must belong to UID 0 with mode 0700.
The receipt must be a regular UID-0 file with mode 0400 and one link.
Both held and freshly opened directory/file descriptors must report a read-only mount through `fstatvfs`.

The reader retains the original directory and receipt descriptors.
Before and after each bounded read, it compares their metadata and the reopened fixed path.
The comparison includes device, inode, type, permissions, owner, group, links, size, modification time and change time.
Every later read must retain the original bytes.
A changed file, missing path, new generation, changed mount or writable boundary causes refusal.
A copied receipt in writable workspace storage cannot create a live reader.

## Original kernel custody

The reader retains original PID, network and user namespace descriptors.
Each revalidation compares them with the current calling thread and PID 1.
The actual network namespace device/inode and PID-1 start ticks must match the receipt.
The effective unprivileged-port boundary must match the receipt and remain at least 1024.
Kernel text reads require procfs and have a 64 KiB bound.
The process ID in procfs must match the current process.

Real/effective UID and all four thread UID fields must be zero.
The calling thread and PID 1 must retain effective `NET_BIND_SERVICE`.
Thread capabilities come from `/proc/thread-self/status`, not another thread's process status.
The current UID/GID maps must each contain only the full identity range `0 0 4294967295`.
Rootless workers and user-namespace remapping are unsupported in this original host-root Docker lane.
No unsafe code, namespace ioctl or new dependency implements these checks.

The same PID-namespace check is essential.
Linux kills the other processes in a PID namespace when its PID 1 exits.
Thus the original worker cannot survive that init and adopt its replacement.
The reader also compares the original start ticks and refuses dead init states.
This uses the [Linux PID namespace contract](https://man7.org/linux/man-pages/man7/pid_namespaces.7.html).
It does not create a general process supervisor or pidfd service.

## Lifetime and limits

`LiveReceipt` has private fields and no Clone, Serialize or Deserialize implementation.
It retains original file/namespace custody, and revalidation failure permanently invalidates that instance.
The facts accessor revalidates first and returns only passive facts.
A serialized response cannot recreate the reader.
A future runtime must retain the original reader and revalidate before its effects.
This component adds no automatic watcher or transport keeper.

Each check is a bounded observation, not an atomic kernel snapshot.
Trusted root on the original host controls publication and the read-only mount.
The reader does not authenticate a provider API or compare a remote caller's saved SSH identity.
It does not prove that Docker preserves a peer address across IPv4/IPv6 publication.
It does not select high ports after refusal or authorize named-source admission.
Actual Gate, provider, owner/public interfaces and complete M2 qualification remain separate outcomes.

## Modules and qualification

`files.rs` owns the fixed path, descriptor metadata and read-only checks.
`kernel.rs` owns bounded Linux readback and original namespace comparisons.
The module root retains custody and invalidates a failed reader.
The reader has no bootstrap capture dependency or environment fallback.
The pure allocation comparison used by the Keeper refuses a different expected allocation.

The [procedure](../testing/procedures/protected-ingress-reader.md) separates pure tests from H03 readback.
Five pure tests cover malformed receipt/metadata, thread capability, identity maps and init parsing.
The actual-root test is ignored by default and belongs only to a reviewed original worker on an owned VM.
Source review or a copied receipt does not qualify H03.
