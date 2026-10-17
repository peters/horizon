"""Return the newest entries of a log for the status line."""


def newest(entries, count):
    """Return the last `count` entries, oldest first."""
    if count <= 0:
        return []
    return entries[len(entries) - count - 1:]
