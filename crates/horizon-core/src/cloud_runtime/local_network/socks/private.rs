//! Immutable `StreamLocal` target for one-service SSH, independent of TCP port reuse.
use super::Proxy;
#[cfg(unix)]
use std::io;
use std::path::Path;

impl Proxy {
    pub(in crate::cloud_runtime::local_network) fn private_path(&self) -> Option<&Path> {
        #[cfg(unix)]
        {
            self.private.as_ref().map(|endpoint| endpoint.path.as_path())
        }
    }
}

#[cfg(unix)]
mod host {
    use super::{Path, Proxy, io};
    use crate::cloud_runtime::local_network::socks::{Gate, Listener, Shared, accept};
    use socket2::Socket;
    use std::{
        fs::{self, Permissions},
        net::{Ipv4Addr, TcpListener},
        os::unix::{
            fs::{MetadataExt, PermissionsExt},
            net::UnixListener,
        },
        path::PathBuf,
        sync::Arc,
        thread,
    };
    use tempfile::TempDir;
    use uuid::Uuid;

    pub(in crate::cloud_runtime::local_network::socks) struct Endpoint {
        // Retained until the exact gateway and supervisor are dropped; never restored.
        _directory: TempDir,
        pub(super) path: PathBuf,
    }

    impl Endpoint {
        fn bind() -> io::Result<(Self, UnixListener)> {
            // Fixed trusted sticky parent, not caller paths or inherited TMPDIR.
            let parent = Path::new("/tmp");
            let metadata = fs::symlink_metadata(parent)?;
            if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o1777 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Private bridge socket parent is unavailable",
                ));
            }
            let directory = tempfile::Builder::new()
                .prefix(&format!("horizon-lnb-{}-", Uuid::new_v4()))
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in(parent)?;
            if fs::symlink_metadata(directory.path())?.permissions().mode() & 0o777 != 0o700 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Private bridge socket directory is not private",
                ));
            }
            let path = directory.path().join("proxy.sock");
            let listener = UnixListener::bind(&path)?;
            fs::set_permissions(&path, Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            Ok((
                Self {
                    _directory: directory,
                    path,
                },
                listener,
            ))
        }
    }

    impl Listener for UnixListener {
        fn next_stream(&self) -> io::Result<Socket> {
            self.accept()
                .map(|(stream, _)| Socket::from(std::os::fd::OwnedFd::from(stream)))
        }
    }

    impl Proxy {
        pub(in crate::cloud_runtime::local_network) fn start_private(gate: Arc<dyn Gate>) -> io::Result<Self> {
            let (private, listener) = Endpoint::bind()?;
            // Preserve the nonsecret receipt shape. No TCP accept loop runs for this mode;
            // SSH targets only the fresh Unix path, never this reserved numeric port.
            let reservation = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
            let port = reservation.local_addr()?.port();
            let shared = Shared::new(gate);
            let worker_shared = Arc::clone(&shared);
            let accept = thread::Builder::new()
                .name("one-service-proxy".into())
                .spawn(move || accept(&listener, &worker_shared))?;
            Ok(Self {
                port,
                shared,
                accept: Some(accept),
                reservation: Some(reservation),
                private: Some(private),
            })
        }
    }
}

#[cfg(unix)]
pub(super) use host::Endpoint;

#[cfg(all(test, target_os = "linux"))]
mod tests;
