//! What every virtual-machine provider shares. A provider that rents one machine with a
//! separate data volume and a reserved address uses these, so none of them reaches into
//! another provider's module for a helper that belongs to neither.
mod spec;

pub use spec::throwaway_public_key;
pub(crate) use spec::{image_registry, registry_host, shared_memory_gb};
