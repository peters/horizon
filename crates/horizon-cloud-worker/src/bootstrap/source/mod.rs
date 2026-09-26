//! Signed, private source transfer. Membership records intent; the independently
//! anchored source record proves publication. Neither grants process admission.
mod storage;
use super::{
    inspection, membership, namespaces,
    recovery::{BOOTSTRAP, Bootstrap, MANIFEST, ROOT, decode},
    runtime::Runtime,
    store::{Store, invalid},
};
use horizon_cloud_protocol::{
    ProjectIdentity,
    bootstrap::RecoveryRequest,
    membership::{Manifest, Receipt, Request, Source, State},
    signed::Action,
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    time::{Duration, Instant},
};
pub(super) use storage::Boundary;
use storage::Tree;

pub(super) fn run(upload: bool) -> io::Result<()> {
    if std::env::args().len() != 2 {
        return Err(invalid());
    }
    let deadline = Instant::now() + Source::WORKER_TIMEOUT;
    let mut input = Input {
        file: std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
            .open("/proc/self/fd/0")?,
        deadline,
    };
    let request = if upload {
        upload_request(&mut input)?
    } else {
        super::recovery::read_request(&mut input)?
    };
    let store = Store::open(Path::new(ROOT))?;
    let runtime = Runtime::captured()?;
    let receipt = prepare(&store, &runtime, &request, deadline, |capabilities| {
        inspection::probe_with_timeout(capabilities, remaining(deadline)?)
    })?;
    if upload {
        import(&store, &runtime, &request, &receipt, &mut input, deadline, &mut |_| {
            Ok(())
        })?;
    }
    remaining(deadline)?;
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

pub(in crate::bootstrap) fn upload_request(input: &mut impl Read) -> io::Result<RecoveryRequest> {
    let mut length = [0; 4];
    input.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length > Source::MAX_REQUEST_BYTES {
        return Err(invalid());
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes)?;
    let request: RecoveryRequest = decode(&bytes)?;
    descriptor(&request, length)?;
    Ok(request)
}

fn descriptor(request: &RecoveryRequest, length: usize) -> io::Result<Source> {
    if length > Source::MAX_REQUEST_BYTES {
        return Err(invalid());
    }
    let Request::ImportSource { descriptor } = serde_json::from_str(&request.payload).map_err(|_| invalid())? else {
        return Err(invalid());
    };
    descriptor.validate().map_err(|_| invalid())?;
    if descriptor.pack.length + descriptor.material.length + 4 + length as u64 > Source::MAX_BYTES {
        return Err(invalid());
    }
    Ok(descriptor)
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    let duration = deadline.saturating_duration_since(Instant::now());
    if duration.is_zero() {
        Err(io::Error::new(io::ErrorKind::TimedOut, "Source deadline expired"))
    } else {
        Ok(duration)
    }
}

struct Input {
    file: File,
    deadline: Instant,
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            remaining(self.deadline)?;
            match self.file.read(bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
}

pub(super) fn prepare(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    deadline: Instant,
    probe: impl FnOnce(&horizon_cloud::Capabilities) -> io::Result<()>,
) -> io::Result<Receipt> {
    remaining(deadline)?;
    let descriptor = descriptor(request, serde_json::to_vec(request)?.len())?;
    let receipt = membership::mutate(store, runtime, request, Action::ImportProjectSource, probe, &mut |_| {
        Ok(())
    })?;
    let manifest = context(store, runtime)?;
    let parent = namespaces::repository(store, &manifest, &receipt.identity)?;
    if let Some(tree) = Tree::open(store, &parent, &receipt, &descriptor, false, deadline)? {
        tree.validate(false)?;
    }
    super::store::same(&parent, &namespaces::repository(store, &manifest, &receipt.identity)?)?;
    Ok(receipt)
}

fn context(store: &Store, runtime: &Runtime) -> io::Result<Manifest> {
    let bootstrap: Bootstrap = decode(&store.read(BOOTSTRAP)?.ok_or_else(invalid)?)?;
    bootstrap.validate(store, runtime)?;
    membership::load(store, &bootstrap).map(|(_, manifest)| manifest)
}

pub(super) fn import(
    store: &Store,
    runtime: &Runtime,
    request: &RecoveryRequest,
    receipt: &Receipt,
    input: &mut impl Read,
    deadline: Instant,
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<()> {
    remaining(deadline)?;
    let manifest = context(store, runtime)?;
    let (next, expected) = manifest
        .next(&request.message, &request.payload)
        .map_err(|_| invalid())?;
    if next != manifest || &expected != receipt || receipt.state != State::Importing {
        return Err(invalid());
    }
    let Request::ImportSource { descriptor } = decode(request.payload.as_bytes())? else {
        return Err(invalid());
    };
    let parent = namespaces::repository(store, &manifest, &receipt.identity)?;
    let verify = || {
        remaining(deadline)?;
        if context(store, runtime)? != manifest {
            return Err(invalid());
        }
        super::store::same(&parent, &namespaces::repository(store, &manifest, &receipt.identity)?)
    };
    let mut tree = Tree::open(store, &parent, receipt, &descriptor, true, deadline)?.ok_or_else(invalid)?;
    tree.receive(input, checkpoint, &verify)?;
    tree.publish(checkpoint, &verify)?;
    verify()?;
    store.sync(MANIFEST)
}

fn entries(manifest: &Manifest) -> io::Result<Vec<(&Receipt, Source)>> {
    manifest
        .operations
        .iter()
        .filter(|entry| entry.receipt.state == State::Importing)
        .filter_map(|entry| match decode::<Request>(entry.payload.as_bytes()) {
            Ok(Request::ImportSource { descriptor }) => Some(Ok((&entry.receipt, descriptor))),
            Ok(Request::ReserveSession { .. }) => None,
            _ => Some(Err(invalid())),
        })
        .collect()
}

pub(super) fn validate(store: &Store, manifest: &Manifest, settled: bool) -> io::Result<()> {
    let deadline = Instant::now() + Source::WORKER_TIMEOUT;
    for (receipt, descriptor) in entries(manifest)? {
        let parent = namespaces::repository(store, manifest, &receipt.identity)?;
        let required = settled
            || manifest
                .members
                .iter()
                .any(|member| member.identity == receipt.identity && member.state == State::Removed);
        match Tree::open(store, &parent, receipt, &descriptor, false, deadline)? {
            Some(tree) => tree.validate(required)?,
            None if required => return Err(invalid()),
            None => {}
        }
        super::store::same(&parent, &namespaces::repository(store, manifest, &receipt.identity)?)?;
    }
    Ok(())
}

pub(super) fn require_settled(store: &Store, manifest: &Manifest, identity: &ProjectIdentity) -> io::Result<()> {
    require_settled_until(store, manifest, identity, Instant::now() + Source::WORKER_TIMEOUT)
}

pub(super) fn require_settled_until(
    store: &Store,
    manifest: &Manifest,
    identity: &ProjectIdentity,
    deadline: Instant,
) -> io::Result<()> {
    remaining(deadline)?;
    for (receipt, descriptor) in entries(manifest)?
        .into_iter()
        .filter(|(receipt, _)| &receipt.identity == identity)
    {
        let parent = namespaces::repository(store, manifest, identity)?;
        Tree::open(store, &parent, receipt, &descriptor, false, deadline)?
            .ok_or_else(invalid)?
            .validate(true)?;
        super::store::same(&parent, &namespaces::repository(store, manifest, identity)?)?;
    }
    remaining(deadline)?;
    Ok(())
}
