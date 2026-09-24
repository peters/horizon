use super::{
    Error, Result,
    journal::{Anchor, LockIdentity, Marker, hash},
    machine::MachineId,
    vault::Vault,
};
use ed25519_dalek::SigningKey;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Registration {
    pub version: u32,
    pub machine: MachineId,
    pub root: PathBuf,
    pub lock_path: PathBuf,
    pub lock_identity: LockIdentity,
    pub marker: Marker,
    #[serde(with = "key_material")]
    pub key: Zeroizing<Vec<u8>>,
    pub committed: Option<Anchor>,
    pub pending: Option<Transition>,
}

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Transition {
    pub previous: Option<Anchor>,
    pub next: Anchor,
}

impl Registration {
    pub(super) fn read(vault: &dyn Vault, marker: &Marker) -> Result<Self> {
        let bytes = vault.read(&marker.registration.to_string())?;
        if bytes.len() > 16 * 1024 {
            return Err(Error::Registration);
        }
        serde_json::from_slice(&bytes).map_err(|_| Error::Registration)
    }

    pub(super) fn write(&self, vault: &dyn Vault) -> Result<()> {
        let bytes = Zeroizing::new(serde_json::to_vec(self).map_err(|_| Error::Registration)?);
        vault.write(&self.marker.registration.to_string(), &bytes)?;
        if vault.read(&self.marker.registration.to_string())?.as_slice() != bytes.as_slice() {
            return Err(Error::Registration);
        }
        Ok(())
    }

    pub(super) fn verify(&self, root: &Path, marker: &Marker, machine: &MachineId) -> Result<SigningKey> {
        if self.version != 1 || self.root != root || self.marker != *marker || self.machine != *machine {
            return Err(Error::Ownership);
        }
        let lock_parent = self.lock_path.parent().ok_or(Error::Ownership)?;
        if !self.lock_path.is_absolute()
            || self.lock_path.starts_with(root)
            || lock_parent.canonicalize()? != lock_parent
        {
            return Err(Error::Ownership);
        }
        if LockIdentity::at_path(&self.lock_path)? != self.lock_identity {
            return Err(Error::Ownership);
        }
        let seed: &[u8; 32] = self.key.as_slice().try_into().map_err(|_| Error::Registration)?;
        let key = SigningKey::from_bytes(seed);
        if hash(key.verifying_key().as_bytes()) != marker.public_key_hash {
            return Err(Error::Ownership);
        }
        if let Some(pending) = &self.pending {
            let generation = match &self.committed {
                Some(anchor) => anchor.generation.checked_add(1).ok_or(Error::Registration)?,
                None => 0,
            };
            if pending.previous != self.committed || pending.next.generation != generation {
                return Err(Error::Registration);
            }
        } else if self.committed.is_none() {
            return Err(Error::Registration);
        }
        Ok(key)
    }
}

mod key_material {
    use serde::{
        Deserializer, Serialize, Serializer,
        de::{Error, SeqAccess, Visitor},
    };
    use zeroize::Zeroizing;

    pub(super) fn serialize<S: Serializer>(value: &Zeroizing<Vec<u8>>, serializer: S) -> Result<S::Ok, S::Error> {
        value.as_slice().serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Zeroizing<Vec<u8>>, D::Error> {
        struct Seed;
        impl<'de> Visitor<'de> for Seed {
            type Value = Zeroizing<Vec<u8>>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a 32-byte signing seed")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
                let mut seed = Zeroizing::new(Vec::with_capacity(32));
                while let Some(byte) = sequence.next_element::<u8>()? {
                    if seed.len() == 32 {
                        return Err(A::Error::custom("invalid signing seed"));
                    }
                    seed.push(byte);
                }
                if seed.len() != 32 {
                    return Err(A::Error::custom("invalid signing seed"));
                }
                Ok(seed)
            }
        }
        deserializer.deserialize_seq(Seed)
    }
}
