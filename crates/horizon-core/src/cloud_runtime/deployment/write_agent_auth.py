import os
from pathlib import Path
import sys
import tempfile

name = sys.argv[1]
if name not in {"anthropic-api-key", "anthropic-workspace"}:
    raise ValueError("Invalid agent credential binding")
value = sys.stdin.buffer.read(256 * 1024 + 1) if len(sys.argv) == 2 else sys.argv[2].encode()
if not value.strip() or len(value) > 256 * 1024:
    raise ValueError("Invalid agent credential value")
root = Path("/workspace/credentials")
root.mkdir(mode=0o700, parents=True, exist_ok=True)
fd, pending = tempfile.mkstemp(dir=root)
try:
    with os.fdopen(fd, "wb") as output:
        output.write(value)
        output.flush()
        os.fsync(output.fileno())
    os.replace(pending, root / name)
    directory = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
finally:
    Path(pending).unlink(missing_ok=True)
