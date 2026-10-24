//! This computer's neighbor (ARP) table: devices it recently exchanged traffic with. Only the
//! addresses leave this module; hardware addresses are read to skip incomplete entries and
//! are never reported.
use super::{Cancellation, stopped};
use std::{io, net::Ipv4Addr};

/// Output read from the system's `arp` command, at most.
#[cfg(any(test, target_os = "macos"))]
const MAX_OUTPUT: u64 = 256 * 1024;
#[cfg(any(test, target_os = "macos"))]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The neighbors' addresses, or why this system cannot list them.
///
/// # Errors
/// Reports a table that cannot be read, or `Interrupted` once the bridge stops.
pub(super) fn read(cancel: &Cancellation) -> io::Result<Vec<Ipv4Addr>> {
    stopped(cancel)?;
    #[cfg(target_os = "linux")]
    {
        Ok(linux(&std::fs::read_to_string("/proc/net/arp")?))
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("/usr/sbin/arp");
        command.arg("-an").env_clear();
        Ok(bsd(&run(command, cancel)?))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(io::Error::other("this system has no neighbor table Horizon can read"))
    }
}

/// Runs the system's `arp`, stopping and reaping it at [`TIMEOUT`] or as soon as the bridge
/// stops. Output past [`MAX_OUTPUT`] is read and discarded, so a long table still lets `arp`
/// finish, and only its start is parsed.
#[cfg(any(test, target_os = "macos"))]
pub(super) fn run(mut command: std::process::Command, cancel: &Cancellation) -> io::Result<String> {
    use std::{io::Read, process::Stdio, time::Instant};
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child.stdout.take();
    let reader = std::thread::Builder::new()
        .name("local-network-neighbors".into())
        .spawn(move || {
            let mut text = Vec::new();
            if let Some(mut output) = output {
                let _ = (&mut output).take(MAX_OUTPUT).read_to_end(&mut text);
                let _ = io::copy(&mut output, &mut io::sink());
            }
            text
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(io::ErrorKind::Interrupted, super::STOPPED));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("the neighbor table took too long to read"));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    if !status.success() {
        return Err(io::Error::other(format!("arp ended with {status}")));
    }
    let text = reader
        .join()
        .map_err(|_| io::Error::other("the neighbor table could not be read"))?;
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// A hardware address that stands for a real, resolved device.
#[cfg(any(test, target_os = "linux", target_os = "macos"))]
fn resolved(hardware: &str) -> bool {
    let digits: Vec<_> = hardware.split([':', '-']).collect();
    digits.len() == 6
        && digits
            .iter()
            .all(|digit| !digit.is_empty() && digit.len() <= 2 && digit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        && !digits.iter().all(|digit| u8::from_str_radix(digit, 16) == Ok(0))
        && !digits.iter().all(|digit| digit.eq_ignore_ascii_case("ff"))
}

/// `/proc/net/arp`: `IP address, HW type, Flags, HW address, Mask, Device`, with flag 0x2 set
/// on complete entries.
#[cfg(any(test, target_os = "linux"))]
pub(super) fn linux(table: &str) -> Vec<Ipv4Addr> {
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            let [address, _, flags, hardware, ..] = fields.as_slice() else {
                return None;
            };
            let flags = u32::from_str_radix(flags.trim_start_matches("0x"), 16).ok()?;
            (flags & 0x2 != 0 && resolved(hardware)).then(|| address.parse().ok())?
        })
        .collect()
}

/// macOS `arp -an`: `? (192.168.1.1) at 0:11:22:33:44:55 on en0 ifscope [ethernet]`.
#[cfg(any(test, target_os = "macos"))]
pub(super) fn bsd(table: &str) -> Vec<Ipv4Addr> {
    table
        .lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once('(')?;
            let (address, rest) = rest.split_once(')')?;
            let hardware = rest.trim_start().strip_prefix("at ")?.split_whitespace().next()?;
            resolved(hardware).then(|| address.parse().ok())?
        })
        .collect()
}
