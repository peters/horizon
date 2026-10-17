use super::{Error, live};
use serde::Deserialize;
use std::{
    net::IpAddr,
    process::{ExitStatus, Stdio},
    sync::Arc,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError, oneshot},
    time::{Instant, sleep_until},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Generation {
    pub id: String,
    pub name: String,
    pub suffix: String,
    pub addresses: Vec<IpAddr>,
}
pub(super) struct Identity {
    pub stable_id: String,
    pub name: String,
    pub tags: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Node {
    #[serde(rename = "StableID")]
    stable_id: String,
    name: String,
    #[serde(default)]
    tags: Vec<String>,
    addresses: Vec<String>,
    #[serde(default)]
    expired: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Who {
    node: Node,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    backend_state: String,
    #[serde(rename = "Self")]
    own: Own,
    current_tailnet: Tailnet,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Own {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "DNSName")]
    name: String,
    #[serde(rename = "TailscaleIPs")]
    addresses: Vec<IpAddr>,
    #[serde(default)]
    expired: bool,
}
#[derive(Deserialize)]
struct Tailnet {
    #[serde(rename = "MagicDNSSuffix")]
    suffix: String,
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Error> {
    if bytes.len() > 64 * 1024 {
        return Err(Error::Encoding);
    }
    serde_json::from_slice(bytes).map_err(|_| Error::Encoding)
}
fn stable(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn dns(value: &str) -> Result<String, Error> {
    let name = value.strip_suffix('.').unwrap_or(value).to_ascii_lowercase();
    if name.len() > 253
        || !name.contains('.')
        || !name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.as_bytes()[0].is_ascii_alphanumeric()
                && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err(Error::Encoding);
    }
    Ok(name)
}
pub(super) fn generation(bytes: &[u8]) -> Result<Generation, Error> {
    let status: Status = decode(bytes)?;
    let name = dns(&status.own.name)?;
    let suffix = dns(&status.current_tailnet.suffix)?;
    if status.backend_state != "Running"
        || status.own.expired
        || !stable(&status.own.id)
        || !name.ends_with(&format!(".{suffix}"))
        || status.own.addresses.is_empty()
        || status.own.addresses.len() > 16
        || status
            .own
            .addresses
            .iter()
            .any(|ip| ip.is_unspecified() || ip.is_loopback() || ip.is_multicast())
    {
        return Err(Error::Custody);
    }
    let mut addresses = status.own.addresses;
    addresses.sort();
    let count = addresses.len();
    addresses.dedup();
    if addresses.len() != count {
        return Err(Error::Encoding);
    }
    Ok(Generation {
        id: status.own.id,
        name,
        suffix,
        addresses,
    })
}
pub(super) fn identity(bytes: &[u8], source: IpAddr) -> Result<Identity, Error> {
    let node = decode::<Who>(bytes)?.node;
    if node.expired
        || !stable(&node.stable_id)
        || node.addresses.is_empty()
        || node.addresses.len() > 16
        || node.tags.len() > 32
    {
        return Err(Error::Custody);
    }
    let mut matched = false;
    for address in node.addresses {
        let (ip, prefix) = address.split_once('/').ok_or(Error::Encoding)?;
        let ip: IpAddr = ip.parse().map_err(|_| Error::Encoding)?;
        if prefix != if ip.is_ipv4() { "32" } else { "128" } {
            return Err(Error::Encoding);
        }
        matched |= ip == source;
    }
    if !matched {
        return Err(Error::Custody);
    }
    for tag in &node.tags {
        let value = tag.strip_prefix("tag:").ok_or(Error::Encoding)?;
        if value.is_empty()
            || value.len() > 63
            || !value.as_bytes()[0].is_ascii_alphabetic()
            || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(Error::Encoding);
        }
    }
    Ok(Identity {
        stable_id: node.stable_id,
        name: dns(&node.name)?,
        tags: node.tags,
    })
}

async fn read(mut pipe: impl AsyncRead + Unpin, max: usize) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    (&mut pipe).take((max + 1) as u64).read_to_end(&mut bytes).await?;
    if bytes.len() > max {
        return Err(Error::Encoding);
    }
    Ok(bytes)
}

pub(super) struct ChildSlot {
    pool: Arc<Semaphore>,
    _permit: OwnedSemaphorePermit,
    reaped: bool,
}
impl ChildSlot {
    pub(super) fn spawned(pool: Arc<Semaphore>, permit: OwnedSemaphorePermit) -> Self {
        Self {
            pool,
            _permit: permit,
            reaped: false,
        }
    }
    pub(super) fn waited(&mut self, result: std::io::Result<ExitStatus>) -> std::io::Result<ExitStatus> {
        if result.is_ok() {
            self.reaped = true;
        }
        result
    }
}
impl Drop for ChildSlot {
    fn drop(&mut self) {
        // Close acknowledgement cannot infer terminal child status from capacity alone.
        // Poison before returning the permit if wait failed or the supervisor unwound.
        if !self.reaped {
            self.pool.close();
        }
    }
}

/// The bounded supervisor keeps its child permit until kill/wait even if its caller drops.
pub(super) async fn run(mut command: Command, until: Instant, children: Arc<Semaphore>) -> Result<Vec<u8>, Error> {
    live(until)?;
    let slot = Arc::clone(&children).try_acquire_owned().map_err(|error| match error {
        TryAcquireError::NoPermits => Error::Busy,
        TryAcquireError::Closed => Error::Custody,
    })?;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut slot = ChildSlot::spawned(children, slot);
    let stdout = child.stdout.take().ok_or(Error::Custody)?;
    let stderr = child.stderr.take().ok_or(Error::Custody)?;
    let (mut reply, received) = oneshot::channel();
    tokio::spawn(async move {
        let result = {
            let collect = async {
                let (out, _, status) = tokio::try_join!(read(stdout, 64 * 1024), read(stderr, 8 * 1024), async {
                    Ok::<_, Error>(slot.waited(child.wait().await)?)
                })?;
                if !status.success() {
                    return Err(Error::Custody);
                }
                Ok(out)
            };
            tokio::select! {
                biased;
                () = reply.closed() => Err(Error::Deadline),
                () = sleep_until(until) => Err(Error::Deadline),
                result = collect => result,
            }
        };
        if result.is_err() {
            let _ = child.kill().await;
            let _ = slot.waited(child.wait().await);
        }
        drop(slot);
        let _ = reply.send(result);
    });
    received.await.map_err(|_| Error::Custody)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::process::ExitStatusExt, time::Duration};

    fn held(pool: &Arc<Semaphore>) -> ChildSlot {
        ChildSlot {
            pool: Arc::clone(pool),
            _permit: Arc::clone(pool).try_acquire_owned().unwrap(),
            reaped: false,
        }
    }

    #[tokio::test]
    async fn unconfirmed_wait_or_unpolled_supervisor_never_produces_a_clean_drain() {
        let pool = Arc::new(Semaphore::new(super::super::MAX_PENDING));
        let mut slot = held(&pool);
        assert!(slot.waited(Err(std::io::Error::other("synthetic wait error"))).is_err());
        drop(slot);
        assert!(pool.is_closed());
        assert!(
            super::super::daemon::drain_children(pool, Instant::now() + Duration::from_secs(1))
                .await
                .is_err()
        );
        let pool = Arc::new(Semaphore::new(super::super::MAX_PENDING));
        let slot = held(&pool);
        let task = tokio::spawn(async move {
            std::future::pending::<()>().await;
            drop(slot);
        });
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(pool.is_closed());
    }

    #[tokio::test]
    async fn supervisor_panic_poison_is_distinct_from_a_waited_nonzero_exit() {
        let pool = Arc::new(Semaphore::new(1));
        let slot = held(&pool);
        let task = tokio::spawn(async move {
            let _slot = slot;
            panic!("synthetic supervisor panic");
        });
        assert!(task.await.unwrap_err().is_panic());
        assert!(pool.is_closed());
        let pool = Arc::new(Semaphore::new(1));
        let mut slot = held(&pool);
        assert!(!slot.waited(Ok(ExitStatus::from_raw(7 << 8))).unwrap().success());
        drop(slot);
        assert!(!pool.is_closed());
        assert_eq!(pool.available_permits(), 1);
    }
}
