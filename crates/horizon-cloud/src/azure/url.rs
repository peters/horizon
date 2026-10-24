//! Validators for every segment an ARM URL is built from, and for the resource ids
//! ARM answers with. A segment that fails here is never sent; an id that fails here
//! is never trusted.

/// Longest resource id accepted from a response.
const RESOURCE_ID_LIMIT: usize = 512;

/// A subscription id as ARM prints it: a lowercase hyphenated UUID, never a display name.
#[must_use]
pub fn valid_subscription_id(value: &str) -> bool {
    lowercase_uuid(value)
}

/// A resource group name: 1 to 90 characters of alphanumerics, `-`, `_`, `.`, `(` and
/// `)`, not ending in `.`.
#[must_use]
pub fn valid_resource_group_name(value: &str) -> bool {
    (1..=90).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'(' | b')'))
        && !value.ends_with('.')
}

/// A leaf resource name as ARM accepts it for compute resources and deployments:
/// 1 to 128 characters of alphanumerics, `-` and `_`.
#[must_use]
pub fn valid_resource_name(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A region name such as `northeurope`: 2 to 64 lowercase alphanumerics, starting
/// with a letter.
#[must_use]
pub fn valid_location(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// The parts of `/subscriptions/{sub}/resourceGroups/{group}/providers/{provider}/{kind}/{name}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceIdParts<'a> {
    pub group: &'a str,
    pub provider: &'a str,
    pub kind: &'a str,
    pub name: &'a str,
}

/// The parts of a resource id in `subscription_id`, or `None` when the id has another
/// shape. The subscription, `resourceGroups` and `providers` segments compare
/// case-insensitively, as ARM does; the group and name must be valid.
#[must_use]
pub fn resource_id_parts<'a>(value: &'a str, subscription_id: &str) -> Option<ResourceIdParts<'a>> {
    if !plain_id(value) {
        return None;
    }
    let mut parts = value.strip_prefix('/')?.split('/');
    parts.next()?.eq_ignore_ascii_case("subscriptions").then_some(())?;
    parts.next()?.eq_ignore_ascii_case(subscription_id).then_some(())?;
    parts.next()?.eq_ignore_ascii_case("resourceGroups").then_some(())?;
    let group = parts.next().filter(|group| valid_resource_group_name(group))?;
    parts.next()?.eq_ignore_ascii_case("providers").then_some(())?;
    let provider = parts.next()?;
    let kind = parts.next()?;
    let name = parts.next().filter(|name| valid_resource_name(name))?;
    parts.next().is_none().then_some(ResourceIdParts {
        group,
        provider,
        kind,
        name,
    })
}

/// `/subscriptions/{sub}/resourceGroups/{group}` for exactly this subscription and group.
#[must_use]
pub fn valid_resource_group_id(value: &str, subscription_id: &str, resource_group: &str) -> bool {
    if !plain_id(value) {
        return false;
    }
    let mut parts = value.strip_prefix('/').unwrap_or_default().split('/');
    parts
        .next()
        .is_some_and(|segment| segment.eq_ignore_ascii_case("subscriptions"))
        && parts
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case(subscription_id))
        && parts
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case("resourceGroups"))
        && parts
            .next()
            .is_some_and(|segment| segment.eq_ignore_ascii_case(resource_group))
        && parts.next().is_none()
}

/// `/subscriptions/{sub}/resourceGroups/{group}/providers/Microsoft.Compute/virtualMachines/{name}`
/// for exactly this subscription, group and VM name.
#[must_use]
pub fn valid_vm_resource_id(value: &str, subscription_id: &str, resource_group: &str, name: &str) -> bool {
    resource_id_parts(value, subscription_id).is_some_and(|parts| {
        parts.group.eq_ignore_ascii_case(resource_group)
            && parts.provider.eq_ignore_ascii_case("Microsoft.Compute")
            && parts.kind.eq_ignore_ascii_case("virtualMachines")
            && parts.name == name
    })
}

/// A VM's `properties.vmId`: a lowercase hyphenated UUID. The resource id is a path
/// that a delete and a new create under the same name keep; this id changes.
#[must_use]
pub fn valid_instance_id(value: &str) -> bool {
    lowercase_uuid(value)
}

fn lowercase_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'),
        })
}

fn plain_id(value: &str) -> bool {
    value.len() <= RESOURCE_ID_LIMIT && !value.chars().any(|c| c.is_control() || c.is_whitespace())
}
