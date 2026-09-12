"""Azure client-off harness modules: manifest and pure verdict, the bounded `az` client and
cleanup authorization. `client_off.py` is the command-line entry point."""
from __future__ import annotations

from . import az, cleanup, manifest, verdict

MODULES = (manifest, verdict, az, cleanup)
