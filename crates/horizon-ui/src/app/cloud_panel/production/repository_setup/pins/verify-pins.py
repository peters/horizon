#!/usr/bin/env python3
"""Checks that .horizon/Dockerfile pins the toolchain versions the repository's own
files name. Horizon writes this file; .horizon/pins.json says where each pin comes from.

Each entry of "pins" names a Dockerfile ARG and one source:
  {"arg": "DOTNET_SDK_VERSION", "file": "global.json", "json": "sdk.version"}
  {"arg": "RUST_VERSION", "file": "rust-toolchain.toml", "toml": "toolchain.channel"}
  {"arg": "MONGODB_VERSION", "file": "Directory.Build.props", "xml": "MongoDBVersion"}
  {"arg": "NODE_VERSION", "file": ".nvmrc", "regex": "^v?(\\S+)", "compare": "prefix"}
"compare": "prefix" accepts a Dockerfile pin that refines the source, such as 22.11.0
for a source that says 22. Run from anywhere: python3 -I .horizon/verify-pins.py
It reads pins.json and the Dockerfile beside it, so a copy of the setup folder, such
as an image's /opt/NAME-horizon, works too; the repository is the folder around it.
Every file it reads must be a regular file inside the repository, outside .git, and
at most 1 MiB; it never follows a path out of the repository."""
import sys

# Run as a script, Python looks for modules beside it first, in .horizon, which the
# setup agent writes: that folder goes before any other module loads. Run it with -I.
if not sys.flags.isolated:
    SCRIPT_FOLDER = sys.path[0]
    sys.path[:] = [entry for entry in sys.path if entry not in ("", SCRIPT_FOLDER)]

import json
import os
import re
import stat
import subprocess
import xml.etree.ElementTree as ElementTree
from pathlib import Path

# Horizon runs its own copy with the .horizon folder as the argument; otherwise the
# setup folder is this script's, wherever the setup was copied.
HORIZON = (Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent).resolve()
ROOT = HORIZON.parent
LIMIT = 1024 * 1024
# A declared regex searches in a child Python that stops after this many seconds.
REGEX_SECONDS = 2
REGEX_SEARCH = (
    "import json, re, sys\n"
    "pattern, text = json.load(sys.stdin)\n"
    "match = re.search(pattern, text, re.MULTILINE)\n"
    "value = None if match is None else match.group(1) if match.groups() else match.group(0)\n"
    "json.dump([match is not None, value], sys.stdout)\n"
)


def in_setup(name):
    """`name` from the repository root, with a .horizon/ at its start read in the setup
    folder: the same file in a repository, and its copy beside the copied script."""
    text = str(name)
    plain = text[2:] if text.startswith("./") else text
    if plain.startswith(".horizon/"):
        return HORIZON.name + "/" + plain[len(".horizon/") :]
    return text


def read(relative):
    """The text of a repository file, refused when it leaves the repository."""
    path = (ROOT / relative).resolve()
    if path != ROOT and ROOT not in path.parents:
        raise ValueError("is outside the repository")
    if inside_git(path):
        raise ValueError("is inside .git")
    # Opened without waiting on a pipe or a device, and read only when regular and small.
    flags = os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(path, flags)
    with os.fdopen(descriptor, "rb") as source:
        status = os.fstat(source.fileno())
        if not stat.S_ISREG(status.st_mode):
            raise ValueError("is not a regular file")
        if status.st_size > LIMIT:
            raise ValueError(f"is larger than {LIMIT} bytes")
        return source.read(LIMIT + 1).decode("utf-8-sig")


def inside_git(path):
    """Whether `path` is in the repository's .git under any spelling the file system
    accepts for it: .GIT where names ignore case, .git. on Windows, or another link."""
    if any(part.rstrip(". ").casefold() == ".git" for part in path.relative_to(ROOT).parts):
        return True
    try:
        metadata = os.stat(ROOT / ".git")
    except OSError:
        return False
    for folder in [path, *path.parents]:
        if folder == ROOT:
            return False
        try:
            if os.path.samestat(os.stat(folder), metadata):
                return True
        except OSError:
            continue
    return False


def without_comments(text):
    """JSON text without the // and /* */ comments that .NET files such as global.json
    allow; text inside strings is kept."""
    out, index, quoted = [], 0, False
    while index < len(text):
        char = text[index]
        if quoted:
            out.append(char)
            if char == "\\" and index + 1 < len(text):
                out.append(text[index + 1])
                index += 1
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
            out.append(char)
        elif text.startswith("//", index):
            end = text.find("\n", index)
            index = len(text) if end < 0 else end
            continue
        elif text.startswith("/*", index):
            end = text.find("*/", index + 2)
            index = len(text) if end < 0 else end + 2
            continue
        else:
            out.append(char)
        index += 1
    return "".join(out)


TOML_ESCAPES = {"b": "\b", "t": "\t", "n": "\n", "f": "\f", "r": "\r", '"': '"', "\\": "\\"}
TOML_BARE_KEY = re.compile(r"[A-Za-z0-9_-]+")


class TomlUnsupported:
    """A value the reader without tomllib cannot read; only a pin that reads it fails."""

    def __init__(self, why):
        self.why = why


def toml_string(text, start):
    """The basic or literal TOML string at text[start], and the index after it."""
    quote, index, value = text[start], start + 1, []
    while index < len(text):
        char = text[index]
        if char == quote:
            return "".join(value), index + 1
        if quote == '"' and char == "\\":
            code = text[index + 1 : index + 2]
            width = {"u": 4, "U": 8}.get(code, 0)
            digits = text[index + 2 : index + 2 + width]
            if code in TOML_ESCAPES:
                value.append(TOML_ESCAPES[code])
            elif width and len(digits) == width and all(c in "0123456789abcdefABCDEF" for c in digits):
                value.append(chr(int(digits, 16)))
            else:
                raise ValueError("a TOML string has an escape that is not valid")
            index += 2 + width
            continue
        value.append(char)
        index += 1
    raise ValueError("a TOML string is not closed")


def toml_key(text, index):
    """The parts of the dotted TOML key at text[index], and the index after it."""
    parts = []
    while True:
        index = len(text) - len(text[index:].lstrip(" \t"))
        if text[index : index + 1] in ("\"", "'"):
            part, index = toml_string(text, index)
        else:
            match = TOML_BARE_KEY.match(text, index)
            if not match:
                raise ValueError("a TOML key is not valid")
            part, index = match.group(0), match.end()
        parts.append(part)
        index = len(text) - len(text[index:].lstrip(" \t"))
        if text[index : index + 1] != ".":
            return parts, index
        index += 1


def toml_line(text, table, data):
    """Reads one line of tables and key = value pairs into `data`; returns the table
    later pairs go in and the delimiter of a multi-line string that begins here."""
    if text.startswith("[["):
        # An array of tables holds nothing a pin can name: its pairs go nowhere.
        return {}, None
    if text.startswith("["):
        parts, index = toml_key(text, 1)
        if text[index : index + 1] != "]" or text[index + 1 :].strip()[:1] not in ("", "#"):
            raise ValueError("a TOML table header is not valid")
        table = data
        for part in parts:
            table = table.setdefault(part, {})
        return table, None
    parts, index = toml_key(text, 0)
    if text[index : index + 1] != "=":
        raise ValueError("a TOML line is not key = value")
    rest, opened = text[index + 1 :].strip(), None
    if rest[:3] in ('"""', "'''"):
        value = TomlUnsupported("a multi-line TOML string needs Python 3.11 or newer")
        opened = None if rest.count(rest[:3]) > 1 else rest[:3]
    elif rest[:1] in ('"', "'"):
        value, end = toml_string(rest, 0)
        if rest[end:].strip()[:1] not in ("", "#"):
            raise ValueError("a TOML string is followed by more than a comment")
    elif rest[:1] in ("[", "{"):
        value = TomlUnsupported("a TOML array or inline table needs Python 3.11 or newer")
    else:
        value = rest.split("#", 1)[0].strip()
    target = table
    for part in parts[:-1]:
        target = target.setdefault(part, {})
    target[parts[-1]] = value
    return table, opened


def load_toml(text):
    """TOML with tomllib (Python 3.11 and newer). Without it, the tables and key = value
    pairs that toolchain files use, read as TOML reads them: strings with their escapes,
    and # starts a comment only outside a string. A line it cannot read, such as an
    element of a multi-line array, adds nothing."""
    try:
        import tomllib
    except ImportError:
        data, closing = {}, None
        table = data
        for line in text.splitlines():
            if closing:
                closing = None if closing in line else closing
                continue
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            try:
                table, closing = toml_line(line, table, data)
            except ValueError:
                continue
        return data
    return tomllib.loads(text)


def source_value(pin):
    text = read(in_setup(pin["file"]))
    if "json" in pin or "toml" in pin:
        if "json" in pin:
            data, keys = json.loads(without_comments(text)), pin["json"]
        else:
            data, keys = load_toml(text), pin["toml"]
        for key in keys.split("."):
            data = data[key]
        if isinstance(data, TomlUnsupported):
            raise ValueError(data.why)
        return str(data)
    if "xml" in pin:
        # A declaration could expand entities without bound; no build file needs one.
        if re.search(r"<!(DOCTYPE|ENTITY)", text, re.IGNORECASE):
            raise ValueError("an XML file with a DOCTYPE or ENTITY declaration is not read")
        # By local name, so a file in the MSBuild namespace reads like one without it.
        values = [
            (node.text or "").strip()
            for node in ElementTree.fromstring(text).iter()
            if isinstance(node.tag, str) and node.tag.rsplit("}", 1)[-1] == pin["xml"]
        ]
        if len(values) != 1:
            raise ValueError(f"expected exactly one <{pin['xml']}>, found {len(values)}")
        return values[0]
    if "regex" in pin:
        found, value = regex_search(str(pin["regex"]), text)
        if not found:
            raise ValueError(f"no match for {pin['regex']!r}")
        if value is None:
            raise ValueError(f"the group of {pin['regex']!r} matched nothing")
        return value
    raise ValueError("names no json, toml, xml or regex source")


def regex_search(pattern, text):
    """Whether `pattern` matches `text`, and its first group or else the whole match.
    The declaration names the pattern, and a pattern such as ^(a+)+$ can backtrack for
    hours on a near match: the search runs in a child Python with a time bound, so a CI
    job that runs this check never hangs on it."""
    re.compile(pattern)  # A malformed pattern fails here, with its own message.
    if not sys.executable:
        raise ValueError("cannot start Python to search with the regex")
    try:
        done = subprocess.run(
            [sys.executable, "-I", "-S", "-c", REGEX_SEARCH],
            input=json.dumps([pattern, text]),
            capture_output=True,
            text=True,
            timeout=REGEX_SECONDS,
            check=False,
        )
    except subprocess.TimeoutExpired:
        raise ValueError(f"{pattern!r} found no answer within {REGEX_SECONDS} s; use a simpler regex") from None
    if done.returncode != 0:
        raise ValueError(f"the search with {pattern!r} failed")
    found, value = json.loads(done.stdout)
    return found, value


def matches(pinned, expected, compare):
    if compare == "prefix":
        return pinned == expected or pinned.startswith(expected + ".") or pinned.startswith(expected + "-")
    return pinned == expected


def instructions(text):
    """The Dockerfile's instructions as Docker reads them: continuation lines joined,
    comment lines dropped, heredoc bodies skipped, with the escape directive honoured."""
    lines = text.splitlines()
    escape = "\\"
    for line in lines:
        directive = re.fullmatch(r"#\s*escape\s*=\s*([\\`])\s*", line.strip(), re.IGNORECASE)
        if directive:
            escape = directive.group(1)
            continue
        if not re.fullmatch(r"#\s*\w+\s*=.*", line.strip()):
            break
    result, current, heredocs = [], "", []
    for line in lines:
        if heredocs:
            # The body ends at the word alone on its line; <<- also drops leading tabs.
            word, dash = heredocs[0]
            if (line.lstrip("\t") if dash else line).rstrip("\r") == word:
                heredocs.pop(0)
            continue
        stripped = line.strip()
        if stripped.startswith("#") or (not current and not stripped):
            continue
        if line.rstrip().endswith(escape):
            current += line.rstrip()[:-1] + " "
            continue
        current += line
        instruction = current.strip()
        current = ""
        result.append(instruction)
        if not re.match(r"ARG\s", instruction, re.IGNORECASE):
            # A heredoc word is any word, quoted or not; a here-string (<<<) is none.
            for dash, quoted, single, bare in re.findall(
                r"(?<!<)<<(?!<)(-?)\s*(?:\"([^\"]+)\"|'([^']+)'|([^\s\"'<>|&;]+))", instruction
            ):
                heredocs.append((quoted or single or bare, bool(dash)))
    if current.strip():
        result.append(current.strip())
    return result


def dockerfile_arguments(text):
    """Every value each ARG name is given, in order, also several in one instruction: a
    stage may declare it again, and every declaration must hold the pin. Also the names
    declared without a default that no global ARG before the first FROM gives one: their
    value is unpinned."""
    arguments, bare, global_defaults, staged = {}, set(), set(), False
    for instruction in instructions(text):
        if re.match(r"FROM\s", instruction, re.IGNORECASE):
            staged = True
            continue
        declaration = re.match(r"ARG\s+(.*)", instruction, re.IGNORECASE | re.DOTALL)
        if not declaration:
            continue
        rest = declaration.group(1).strip()
        tokens = re.findall(r"[A-Za-z0-9_]+=(?:\"[^\"]*\"|'[^']*'|\S+)|\S+", rest)
        if all(re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*(=.*)?", token, re.DOTALL) for token in tokens):
            for token in tokens:
                name, equals, value = token.partition("=")
                if not equals:
                    bare.add(name)
                    continue
                arguments.setdefault(name, []).append(value.strip("\"'"))
                if not staged:
                    global_defaults.add(name)
        else:
            # Docker has no comment after an instruction: text it cannot read as more
            # declarations, such as "# pinned", stays part of the value, so it fails.
            whole = re.match(r"([A-Za-z0-9_]+)=(.*)", rest, re.DOTALL)
            if whole:
                arguments.setdefault(whole.group(1), []).append(whole.group(2).strip())
    return arguments, bare - global_defaults


def one_line(text):
    """Text from the repository on one line: a newline or another control character is
    shown escaped, so no value can start a line of its own in a build log."""
    return "".join(char if char.isprintable() else repr(char)[1:-1] for char in str(text))


def main():
    # The Dockerfiles the profiles build, the default profile's first: Horizon names
    # them; in CI the declaration's "dockerfiles" does, else .horizon/Dockerfile.
    try:
        # As Horizon's own check does: the setup folder and the declaration are no links.
        if len(sys.argv) < 2 and os.path.islink(Path(__file__).absolute().parent):
            raise ValueError("the folder of this script is a link, not the repository's .horizon")
        if not stat.S_ISREG(os.lstat(HORIZON / "pins.json").st_mode):
            raise ValueError(".horizon/pins.json is not a regular file")
        declaration = json.loads(read(in_setup(".horizon/pins.json")))
        # Only version 1 is understood: another format fails instead of reading as it.
        if not isinstance(declaration, dict) or declaration.get("version") != 1:
            raise ValueError(".horizon/pins.json must be an object with \"version\": 1")
        pins = declaration.get("pins", [])
        names = sys.argv[2:] or declaration.get("dockerfiles") or [".horizon/Dockerfile"]
        dockerfiles = [(name, *dockerfile_arguments(read(in_setup(name)))) for name in names]
    except (OSError, ValueError, AttributeError, TypeError) as error:
        print(one_line(f"Cannot read the pins or the Dockerfile: {error}"), file=sys.stderr)
        return 1
    problems, checked = [], []
    for pin in pins if isinstance(pins, list) else [None]:
        if not isinstance(pin, dict):
            problems.append(f"{pin!r}: a pin is an object with arg and file")
            continue
        arg = str(pin.get("arg", ""))
        try:
            expected = source_value(pin)
        except Exception as error:  # A malformed pin fails the check; it never stops it.
            problems.append(f"{arg}: cannot read {pin.get('file')}: {error}")
            continue
        compare = pin.get("compare", "exact")
        # A pin may belong to one profile's Dockerfile only, such as a GPU toolchain.
        declared = [(name, pinned) for name, arguments, _ in dockerfiles for pinned in arguments.get(arg, [])]
        if not declared:
            problems.append(f"{arg}: no listed Dockerfile has ARG {arg}=...")
            continue
        wrong = [
            (name, pinned)
            for name, arguments, _ in dockerfiles
            for pinned in arguments.get(arg, [])
            if not matches(pinned, expected, compare)
        ]
        for name, _, unpinned in dockerfiles:
            if arg in unpinned:
                problems.append(f"{arg}: {name} declares ARG {arg} without a default, and no global ARG gives one")
                wrong.append((name, ""))
        for name, pinned in wrong:
            if pinned:
                problems.append(f"{arg}: {name} pins {pinned}, {pin['file']} says {expected}")
        if not wrong:
            checked.append(f"{arg}={declared[0][1]}")
    if problems:
        # Each problem is one line that starts with "pin", whatever the values hold.
        lines = "\n".join(f"  pin {one_line(problem)}" for problem in problems)
        print("The Dockerfile pins differ from the repository:\n" + lines, file=sys.stderr)
        print("Update the pins, or run Update setup from CI in Horizon's New cloud.", file=sys.stderr)
        return 1
    print("Horizon pins match: " + one_line(", ".join(checked) or "no pins declared"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
