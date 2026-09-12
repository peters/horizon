"""Azure client-off harness modules: manifest and pure verdict, the bounded `az` client, the
restricted observer channel, the mutation phases and cleanup authorization. `client_off.py`
is the command-line entry point."""

from . import az, cleanup, manifest, observer, phases, verdict  # noqa: E402

MODULES = (manifest, verdict, az, observer, phases, cleanup)
