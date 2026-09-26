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
    let mut input = Input {
        file: std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits().cast_signed())
            .open("/proc/self/fd/0")?,
        deadline: Instant::now() + Duration::from_secs(180),
    };
    let request = if upload {
        let mut length = [0; 4];
        input.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length > 64 * 1024 {
            return Err(invalid());
        }
        let mut bytes = vec![0; length];
        input.read_exact(&mut bytes)?;
        decode(&bytes)?
    } else {
        super::recovery::read_request(&mut input)?
    };
    let store = Store::open(Path::new(ROOT))?;
    let runtime = Runtime::captured()?;
    let receipt = prepare(&store, &runtime, &request, inspection::probe)?;
    if upload {
        import(&store, &runtime, &request, &receipt, &mut input, &mut |_| Ok(()))?;
    }
    serde_json::to_writer(io::stdout().lock(), &receipt)?;
    io::stdout().lock().write_all(b"\n")
}

struct Input {
    file: File,
    deadline: Instant,
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            if Instant::now() >= self.deadline {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "Source deadline expired"));
            }
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
    probe: impl FnOnce(&horizon_cloud::Capabilities) -> io::Result<()>,
) -> io::Result<Receipt> {
    let receipt = membership::mutate(store, runtime, request, Action::ImportProjectSource, probe, &mut |_| {
        Ok(())
    })?;
    let manifest = context(store, runtime)?;
    let Request::ImportSource { descriptor } = decode(request.payload.as_bytes())? else {
        return Err(invalid());
    };
    let parent = namespaces::repository(store, &manifest, &receipt.identity)?;
    if let Some(tree) = Tree::open(store, &parent, &receipt, &descriptor, false)? {
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
    checkpoint: &mut impl FnMut(Boundary) -> io::Result<()>,
) -> io::Result<()> {
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
        if context(store, runtime)? != manifest {
            return Err(invalid());
        }
        super::store::same(&parent, &namespaces::repository(store, &manifest, &receipt.identity)?)
    };
    let mut tree = Tree::open(store, &parent, receipt, &descriptor, true)?.ok_or_else(invalid)?;
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
        .map(|entry| {
            let Request::ImportSource { descriptor } = decode(entry.payload.as_bytes())? else {
                return Err(invalid());
            };
            Ok((&entry.receipt, descriptor))
        })
        .collect()
}

pub(super) fn validate(store: &Store, manifest: &Manifest, settled: bool) -> io::Result<()> {
    for (receipt, descriptor) in entries(manifest)? {
        let parent = namespaces::repository(store, manifest, &receipt.identity)?;
        let required = settled
            || manifest
                .members
                .iter()
                .any(|member| member.identity == receipt.identity && member.state == State::Removed);
        match Tree::open(store, &parent, receipt, &descriptor, false)? {
            Some(tree) => tree.validate(required)?,
            None if required => return Err(invalid()),
            None => {}
        }
        super::store::same(&parent, &namespaces::repository(store, manifest, &receipt.identity)?)?;
    }
    Ok(())
}

pub(super) fn require_settled(store: &Store, manifest: &Manifest, identity: &ProjectIdentity) -> io::Result<()> {
    for (receipt, descriptor) in entries(manifest)?
        .into_iter()
        .filter(|(receipt, _)| &receipt.identity == identity)
    {
        let parent = namespaces::repository(store, manifest, identity)?;
        Tree::open(store, &parent, receipt, &descriptor, false)?
            .ok_or_else(invalid)?
            .validate(true)?;
        super::store::same(&parent, &namespaces::repository(store, manifest, identity)?)?;
    }
    Ok(())
}
