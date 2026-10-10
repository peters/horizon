//! Linux boot identity is positive evidence that an earlier boot's processes cannot remain alive.
use uuid::Uuid;

/// Return a bounded kernel boot UUID. Unsupported or unavailable hosts retain uncertainty.
#[must_use]
pub fn current() -> Option<Uuid> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open("/proc/sys/kernel/random/boot_id")
            .ok()?
            .take(65)
            .read_to_end(&mut bytes)
            .ok()?;
        parse(&bytes)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

#[cfg(any(target_os = "linux", test))]
fn parse(bytes: &[u8]) -> Option<Uuid> {
    let value = std::str::from_utf8(bytes).ok()?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    if value.len() != 36 {
        return None;
    }
    let id = Uuid::parse_str(value).ok()?;
    (!id.is_nil() && id.hyphenated().to_string() == value).then_some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_identity_requires_one_canonical_nonzero_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(parse(format!("{id}\n").as_bytes()), Some(id));
        assert_eq!(parse(id.to_string().as_bytes()), Some(id));
        for value in [
            String::new(),
            Uuid::nil().to_string(),
            format!("{id}\n\n"),
            format!(" {id}"),
            id.simple().to_string(),
            "x".repeat(65),
        ] {
            assert_eq!(parse(value.as_bytes()), None);
        }
    }
}
