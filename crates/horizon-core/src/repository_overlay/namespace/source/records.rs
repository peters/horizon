use super::Error;
use git2::Oid;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Kind {
    Directory,
    File,
    Executable,
    Link,
}

pub(super) struct Entry<'a> {
    pub name: &'a str,
    pub object: Oid,
    pub kind: Kind,
}

pub(super) struct Tree<'a> {
    remaining: &'a [u8],
}

impl<'a> Tree<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    pub(super) fn next(&mut self) -> Result<Option<Entry<'a>>, Error> {
        if self.remaining.is_empty() {
            return Ok(None);
        }
        let space = self.remaining.iter().position(|b| *b == b' ').ok_or(Error::Object)?;
        let kind = match &self.remaining[..space] {
            b"40000" | b"040000" => Kind::Directory,
            b"100644" => Kind::File,
            b"100755" => Kind::Executable,
            b"120000" => Kind::Link,
            _ => return Err(Error::UnsupportedNode),
        };
        let rest = &self.remaining[space + 1..];
        let nul = rest.iter().position(|b| *b == 0).ok_or(Error::Object)?;
        let name = std::str::from_utf8(&rest[..nul]).map_err(|_| Error::UnsupportedNode)?;
        if name.is_empty() || name.contains('/') {
            return Err(Error::UnsupportedNode);
        }
        let tail = &rest[nul + 1..];
        let object = Oid::from_bytes(tail.get(..20).ok_or(Error::Object)?).map_err(|_| Error::Object)?;
        self.remaining = &tail[20..];
        Ok(Some(Entry { name, object, kind }))
    }
}

pub(super) fn commit_tree(bytes: &[u8]) -> Result<Oid, Error> {
    let boundary = bytes.windows(2).position(|pair| pair == b"\n\n").ok_or(Error::Object)?;
    let mut lines = bytes[..boundary].split(|b| *b == b'\n');
    let tree = oid_line(lines.next().ok_or(Error::Object)?, b"tree ")?;
    let mut line = lines.next().ok_or(Error::Object)?;
    while line.starts_with(b"parent ") {
        oid_line(line, b"parent ")?;
        line = lines.next().ok_or(Error::Object)?;
    }
    signature(line, b"author ")?;
    line = lines.next().ok_or(Error::Object)?;
    while line.starts_with(b"author ") {
        signature(line, b"author ")?;
        line = lines.next().ok_or(Error::Object)?;
    }
    signature(line, b"committer ")?;
    let mut extra = false;
    for line in lines {
        if line.contains(&0) {
            return Err(Error::Object);
        }
        if line.starts_with(b" ") {
            if !extra {
                return Err(Error::Object);
            }
        } else {
            let space = line.iter().position(|b| *b == b' ').ok_or(Error::Object)?;
            let key = &line[..space];
            if key.is_empty()
                || !key.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
                || matches!(key, b"tree" | b"parent" | b"author" | b"committer")
            {
                return Err(Error::Object);
            }
            extra = true;
        }
    }
    Ok(tree)
}

fn oid_line(line: &[u8], prefix: &[u8]) -> Result<Oid, Error> {
    let hex = line
        .strip_prefix(prefix)
        .filter(|hex| hex.len() == 40)
        .ok_or(Error::Object)?;
    let text = std::str::from_utf8(hex).map_err(|_| Error::Object)?;
    Oid::from_str(text).map_err(|_| Error::Object)
}

fn signature(line: &[u8], prefix: &[u8]) -> Result<(), Error> {
    let value = line.strip_prefix(prefix).ok_or(Error::Object)?;
    if value.contains(&0) {
        return Err(Error::Object);
    }
    let close = value.iter().rposition(|b| *b == b'>').ok_or(Error::Object)?;
    let open = value[..close].iter().position(|b| *b == b'<').ok_or(Error::Object)?;
    if open == 0 || value[open - 1] != b' ' || value[open + 1..close].contains(&b'<') {
        return Err(Error::Object);
    }
    let suffix = std::str::from_utf8(&value[close + 1..]).map_err(|_| Error::Object)?;
    let mut fields = suffix.strip_prefix(' ').ok_or(Error::Object)?.split(' ');
    fields
        .next()
        .ok_or(Error::Object)?
        .parse::<i64>()
        .map_err(|_| Error::Object)?;
    let zone = fields.next().ok_or(Error::Object)?.as_bytes();
    if zone.len() != 5
        || !matches!(zone[0], b'+' | b'-')
        || !zone[1..].iter().all(u8::is_ascii_digit)
        || fields.next().is_some()
    {
        return Err(Error::Object);
    }
    Ok(())
}
