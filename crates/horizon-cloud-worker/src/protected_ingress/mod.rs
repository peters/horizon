//! Live root-only readback of the fixed protected host receipt, never exposure approval.
mod files;
mod kernel;
use horizon_cloud_protocol::exposure::ingress::{Receipt, ReceiptFacts};
use std::io;

/// Original in-process file and namespace custody. Serialized facts cannot recreate it.
/// A failed revalidation permanently invalidates this reader.
#[must_use]
pub struct LiveReceipt {
    files: files::ReceiptFiles,
    kernel: kernel::Custody,
    receipt: Receipt,
    bytes: Vec<u8>,
    allocation: String,
    valid: bool,
}

impl LiveReceipt {
    /// Read only the fixed original host mount and current kernel boundary.
    ///
    /// # Errors
    /// Refuses non-root/remapped callers, unsafe metadata or mismatched live identity.
    /// No path, receipt JSON or requested allocation can substitute for these inputs.
    pub fn capture() -> io::Result<Self> {
        kernel::require_root()?;
        let files = files::ReceiptFiles::open()?;
        let bytes = files.read()?;
        let receipt = decode(&bytes)?;
        let kernel = kernel::Custody::capture()?;
        kernel.validate(receipt.facts())?;
        let allocation = receipt.facts().allocation.clone();
        let mut live = Self {
            files,
            kernel,
            receipt,
            bytes,
            allocation,
            valid: true,
        };
        live.revalidate()?;
        Ok(live)
    }

    /// Recheck the original file, allocation and current kernel boundary.
    ///
    /// # Errors
    /// Refuses changed or lost original custody; it never adopts a successor.
    pub fn revalidate(&mut self) -> io::Result<()> {
        if !self.valid {
            return Err(refused());
        }
        let result = self.check();
        if result.is_err() {
            self.valid = false;
        }
        result
    }

    /// Export passive comparison facts after revalidation; no transport is created.
    ///
    /// # Errors
    /// Returns the same custody refusals as [`Self::revalidate`].
    pub fn facts(&mut self) -> io::Result<&ReceiptFacts> {
        self.revalidate()?;
        Ok(self.receipt.facts())
    }

    fn check(&self) -> io::Result<()> {
        self.kernel.validate(self.receipt.facts())?;
        match_allocation(self.receipt.facts(), &self.allocation)?;
        if self.files.read()? != self.bytes {
            return Err(refused());
        }
        self.kernel.validate(self.receipt.facts())
    }
}

fn decode(bytes: &[u8]) -> io::Result<Receipt> {
    Receipt::from_json(bytes).map_err(|_| refused())
}

fn match_allocation(facts: &ReceiptFacts, expected: &str) -> io::Result<()> {
    if facts.allocation != expected {
        return Err(refused());
    }
    Ok(())
}

fn refused() -> io::Error {
    io::Error::other("Protected host ingress receipt is unavailable or changed")
}

#[cfg(test)]
mod tests;
