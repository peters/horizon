//! Host allocation records and shared identity/placement contracts.
pub mod legacy;

pub use horizon_cloud_protocol::{
    AllocationId, BindingError, ControllerId, ExistingWorker, Placement, PlacementBinding, ProjectId, ProjectIdentity,
    SharingMode,
};
