# Tailscale Serve Unix PROXY boundary

The worker runtime uses a narrowly patched Tailscale v1.102.5 build. The CLI permits
`serve --tcp=<virtual-port> --proxy-protocol=1 unix:/<socket>`; the existing daemon
Unix dial writes a PROXY v1 header before application bytes. The header destination
uses the node address of the source family and the original virtual Serve port.
Unix v2 refuses before forwarding. Existing TCP v1/v2, Unix without PROXY,
userspace HTTP/SOCKS proxies, enrollment and networking remain available.

Application bytes and PROXY-looking text are untrusted. Generic HTTP/SOCKS proxies
use TCP destinations. A loopback TCP backend and a reverse descriptor owned by the
same daemon cannot distinguish Serve from those proxies. They must not grant
named-source authority. This runtime addition provides a Unix transport that those
TCP-only proxy destinations cannot select. It grants no authority by itself.

The consumer must create and retain a root-owned 0700 directory and its original
Unix listener. Before accepting identity it must check the accepted stream's kernel
peer credentials against the original pinned daemon, including PID/start/executable
and namespace custody. It must retain and revalidate the original directory,
listener, daemon and service generation through cancellation and cleanup. It must
parse one bounded v1 header, compare the actual virtual port and node addresses,
and perform current identity checks through that same daemon. Serialized path,
header, WhoIs result, version string or saved metadata cannot replace that custody.
A TCP bridge, fallback loopback listener or arbitrary supplied socket path would
break this boundary. The root controller must exclusively own the Serve route
and bind its virtual ingress to this original listener with PROXY v1 enabled.
An unrelated privileged route to the same socket, including a route without
PROXY framing, is not a trusted ingress. A privileged local attacker remains
outside this isolation.

This prerequisite changes no Worker exposure feature defaults, Core/UI/provider
route, policy, public MCP or CLI control. It does not deliver M2 or qualify an old
TCP-based Producer. The subsequent Unix consumer needs independent first/final
real userspace and kernel tests, including same-daemon generic-proxy forgery,
wrong-daemon/local-user refusal and teardown of original streams and listeners.

The exact source archive and Go image index are pinned in the runtime recipe.
`examples/cloud-worker/tailscale` owns the small upstream patch and shared builder.
The two native image jobs test the affected upstream packages and ship only the
resulting binaries and licensing. Build success, compiled support and a published
digest do not prove enrollment, runtime approval or connection identity.
