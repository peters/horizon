use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{Error, Result, Topology};

pub const ALPN: &[u8] = b"horizon-net/1";
const MAX_MESSAGE: usize = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Request {
    Probe { network: String },
    Connect { network: String, service: String },
    Update { topology: Topology },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Response {
    pub accepted: bool,
}

pub(crate) async fn write<T: Serialize>(stream: &mut (impl AsyncWrite + Unpin), value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    let length = u32::try_from(bytes.len()).map_err(|_| Error::MessageTooLarge)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(Error::MessageTooLarge);
    }
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(&bytes).await?;
    stream.flush().await?;
    Ok(())
}

pub(crate) async fn read<T: DeserializeOwned>(stream: &mut (impl AsyncRead + Unpin)) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    if length > MAX_MESSAGE {
        return Err(Error::MessageTooLarge);
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
