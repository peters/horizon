use horizon_device::{DeviceError, ImageDimensions, ResizeBackend, Result, Target};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{runtime::Runtime, task::JoinHandle};
use vnc::{PixelFormat, ResizeError, VncClient, VncConnector, VncEncoding, VncEvent};

pub fn connect(target: &Target) -> Result<Box<dyn ResizeBackend>> {
    connect_with_timeout(target, Duration::from_secs(5))
}

fn connect_with_timeout(target: &Target, deadline: Duration) -> Result<Box<dyn ResizeBackend>> {
    let address = target
        .desktop_resize
        .vnc_address
        .ok_or_else(|| DeviceError::Unsupported("no VNC resize endpoint".into()))?;
    if !address.ip().is_loopback() || address.port() == 0 {
        return Err(DeviceError::Invalid(
            "resize endpoint requires numeric loopback and a nonzero port".into(),
        ));
    }
    // The shared runner calls factories in a blocking worker, outside its async runtime.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(unavailable)?;
    let client = runtime.block_on(async {
        tokio::time::timeout(deadline, async {
            let stream = tokio::net::TcpStream::connect(address).await.map_err(unavailable)?;
            stream.set_nodelay(true).map_err(unavailable)?;
            VncConnector::new(stream)
                .set_auth_method(async { Err(vnc::VncError::NoPassword) })
                .allow_shared(true)
                .set_pixel_format(PixelFormat::rgba())
                .add_encoding(VncEncoding::Raw)
                .add_encoding(VncEncoding::DesktopSizePseudo)
                .add_encoding(VncEncoding::ExtendedDesktopSizePseudo)
                .build()
                .map_err(unavailable)?
                .try_start()
                .await
                .map_err(unavailable)?
                .finish()
                .map_err(unavailable)
        })
        .await
        .map_err(|_| DeviceError::ResizeTimeout { uncertain: false })?
    })?;
    let receiver = client.clone();
    let response_seen = Arc::new(AtomicBool::new(false));
    let response = Arc::clone(&response_seen);
    let drain = runtime.spawn(async move {
        let mut server_init_seen = false;
        while let Ok(event) = receiver.recv_event().await {
            if matches!(event, VncEvent::Error(_)) {
                break;
            }
            match event {
                // The decoder queues ServerInit before framebuffer events.
                VncEvent::SetResolution(_) if !server_init_seen => server_init_seen = true,
                VncEvent::RawImage(_, _) | VncEvent::SetResolution(_) | VncEvent::DesktopUpdate(_) => {
                    response.store(true, Ordering::Release);
                }
                _ => {}
            }
        }
    });
    let negotiated = runtime.block_on(async {
        tokio::time::timeout(deadline, async {
            while client.desktop_layout().is_none() && !drain.is_finished() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
    });
    if negotiated.is_err() && !response_seen.load(Ordering::Acquire) {
        drain.abort();
        return Err(DeviceError::ResizeTimeout { uncertain: false });
    }
    if drain.is_finished() {
        return Err(DeviceError::Unavailable("VNC negotiation disconnected".into()));
    }
    Ok(Box::new(VncResize { runtime, client, drain }))
}

struct VncResize {
    runtime: Runtime,
    client: VncClient,
    drain: JoinHandle<()>,
}
impl ResizeBackend for VncResize {
    fn supported(&self) -> Result<bool> {
        if self.drain.is_finished() {
            return Err(DeviceError::Unavailable("VNC resize connection disconnected".into()));
        }
        Ok(self.client.desktop_layout().is_some())
    }
    fn dimensions(&self) -> Result<ImageDimensions> {
        self.supported()?;
        self.client
            .desktop_layout()
            .map(|layout| ImageDimensions {
                width: u32::from(layout.width),
                height: u32::from(layout.height),
            })
            .ok_or_else(|| DeviceError::Unsupported("server has not advertised desktop resizing".into()))
    }
    fn resize(&mut self, requested: ImageDimensions) -> Result<ImageDimensions> {
        let width =
            u16::try_from(requested.width).map_err(|_| DeviceError::Invalid("width exceeds protocol limit".into()))?;
        let height = u16::try_from(requested.height)
            .map_err(|_| DeviceError::Invalid("height exceeds protocol limit".into()))?;
        let layout = self
            .runtime
            .block_on(self.client.resize_desktop(width, height))
            .map_err(|error| resize_error(&error))?;
        Ok(ImageDimensions {
            width: u32::from(layout.width),
            height: u32::from(layout.height),
        })
    }
}
impl Drop for VncResize {
    fn drop(&mut self) {
        self.drain.abort();
        let _ = self
            .runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(1), self.client.close()).await });
    }
}
fn unavailable(error: impl std::fmt::Display) -> DeviceError {
    DeviceError::Unavailable(error.to_string())
}
fn resize_error(error: &ResizeError) -> DeviceError {
    match error {
        ResizeError::Unsupported | ResizeError::UnsupportedLayout => DeviceError::Unsupported(error.to_string()),
        ResizeError::InvalidDimensions => DeviceError::Invalid(error.to_string()),
        ResizeError::Busy | ResizeError::Disconnected => DeviceError::Unavailable(error.to_string()),
        ResizeError::Denied(_) => DeviceError::ResizeDenied(error.to_string()),
        ResizeError::DispatchTimeout => DeviceError::ResizeTimeout { uncertain: false },
        ResizeError::Timeout => DeviceError::ResizeTimeout { uncertain: true },
        ResizeError::Uncertain => DeviceError::ResizeUncertain(error.to_string()),
    }
}

#[cfg(test)]
mod tests;
