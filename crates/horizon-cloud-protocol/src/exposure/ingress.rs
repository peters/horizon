//! Fixed protected-container geometry and passive host readback facts.
//!
//! Neither parsing a receipt nor calculating a port proves that a host published
//! it, that a peer address survives forwarding, or that a caller owns a worker.
mod receipt;
pub use receipt::{
    Container, Error, MAX_RECEIPT_BYTES, Namespace, Protocol, PublishedPort, RECEIPT_VERSION, Receipt, ReceiptFacts,
};

use super::{RESERVED_INGRESS_END, RESERVED_INGRESS_START};

pub const LISTENER_START: u16 = 600;
pub const LISTENER_END: u16 = 631;
pub const BACKEND_START: u16 = 632;
pub const BACKEND_END: u16 = 663;
pub const REQUIRED_UNPRIVILEGED_PORT_START: u16 = 1024;

/// Physical wildcard listener for a logical external ingress port.
#[must_use]
pub const fn listener_port(ingress: u16) -> Option<u16> {
    if ingress >= RESERVED_INGRESS_START && ingress <= RESERVED_INGRESS_END {
        Some(LISTENER_START + ingress - RESERVED_INGRESS_START)
    } else {
        None
    }
}

/// Physical loopback backend for a logical tailnet virtual port; never published.
#[must_use]
pub const fn backend_port(virtual_port: u16) -> Option<u16> {
    if virtual_port >= RESERVED_INGRESS_START && virtual_port <= RESERVED_INGRESS_END {
        Some(BACKEND_START + virtual_port - RESERVED_INGRESS_START)
    } else {
        None
    }
}

#[must_use]
pub const fn is_physical_port(port: u16) -> bool {
    port >= LISTENER_START && port <= BACKEND_END
}

#[cfg(test)]
mod tests;
