use super::process::{Session, failed};
use git2::{ObjectType, Oid};
use std::io::{self, Read};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Header {
    pub oid: Oid,
    pub kind: ObjectType,
    pub bytes: u64,
}
impl Header {
    pub(super) fn parse(line: &[u8], oid: Oid) -> Option<Self> {
        let text = std::str::from_utf8(line).ok()?;
        let mut fields = text.split(' ');
        if fields.next()? != oid.to_string() {
            return None;
        }
        let kind = match fields.next()? {
            "blob" => ObjectType::Blob,
            "tree" => ObjectType::Tree,
            "commit" => ObjectType::Commit,
            "tag" => ObjectType::Tag,
            _ => return None,
        };
        let size = fields.next()?;
        if size.is_empty()
            || !size.bytes().all(|b| b.is_ascii_digit())
            || (size.len() > 1 && size.starts_with('0'))
            || fields.next().is_some()
        {
            return None;
        }
        Some(Self {
            oid,
            kind,
            bytes: size.parse().ok()?,
        })
    }
}

pub(super) struct ObjectReader<'s, 'a> {
    session: &'s mut Session<'a>,
    header: Header,
    remaining: u64,
    started: bool,
    complete: bool,
}
impl<'s, 'a> ObjectReader<'s, 'a> {
    pub(super) fn new(session: &'s mut Session<'a>, header: Header) -> Self {
        Self {
            session,
            header,
            remaining: header.bytes,
            started: false,
            complete: false,
        }
    }
    fn read_payload(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if !self.started {
            self.session.request("contents", self.header.oid)?;
            if self.session.header(self.header.oid)? != self.header {
                return Err(failed());
            }
            self.started = true;
        }
        if self.remaining == 0 {
            let mut delimiter = [0];
            self.session.exact(&mut delimiter)?;
            if delimiter != *b"\n" {
                return Err(failed());
            }
            self.complete = true;
            return Ok(0);
        }
        let limit = usize::try_from(self.remaining.min(buffer.len() as u64)).map_err(|_| failed())?;
        let n = self.session.read(&mut buffer[..limit])?;
        if n == 0 {
            return Err(failed());
        }
        self.remaining -= n as u64;
        Ok(n)
    }
}
impl Read for ObjectReader<'_, '_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.complete {
            return Ok(0);
        }
        let result = self.read_payload(buffer);
        if result.is_err() {
            self.session.poison();
        }
        result
    }
}
impl Drop for ObjectReader<'_, '_> {
    fn drop(&mut self) {
        if !self.complete {
            self.session.poison();
        }
    }
}
