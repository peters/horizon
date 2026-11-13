//! Why the last attempt at the link of this computer did not open it.
//!
//! The words of the `ssh` child and the error of a socket reach the log, and a
//! person who works from a place with a changing address reads the sidebar. One
//! attempt therefore leaves one of these reasons behind, and the sidebar line
//! `Server away` carries it as a hover and the Server tab as a small line under
//! its state.
//!
//! A reason is one fixed sentence. It carries no address, no name of a computer
//! and no part of a secret, so the window can show it while the log line stays in
//! the log.

use std::{
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use horizon_core::place::SSH_CONNECTION_FAILURE;

/// Why the last attempt at the link did not open it, as a person reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkRefusal {
    /// The address answered nothing: a refused link, a timeout, no route.
    Address,
    /// The name of the server did not resolve, or answered no address.
    Name,
    /// The key of the server is not the key that this computer knows.
    HostKey,
    /// The login of this computer was refused.
    Login,
    /// The far side has no command for the link: `ssh` ended with 127.
    Command,
    /// The link was open and the server program stopped answering.
    Silent,
    /// Nothing that this computer can name. The log holds the words.
    Other,
}

impl LinkRefusal {
    /// The one sentence that a person reads.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Address => "The server did not answer at its address.",
            Self::Name => "The name of the server did not resolve.",
            Self::HostKey => "The key of the server is not the key that this computer knows.",
            Self::Login => "The server refused the login of this computer.",
            Self::Command => "The server has no such command for the link.",
            Self::Silent => "The server program did not answer.",
            Self::Other => "The link to the server failed. The log holds the words of the attempt.",
        }
    }

    /// The stable code of the reason, for a log line and a tool answer.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Address => "address",
            Self::Name => "name",
            Self::HostKey => "host_key",
            Self::Login => "login",
            Self::Command => "command",
            Self::Silent => "silent",
            Self::Other => "other",
        }
    }
}

/// The reason that one line of the standard error of `ssh` names.
///
/// The lines are the ones that OpenSSH writes, in the words of its own messages:
/// it prints the same line for every link it cannot open, and no line of a
/// person's command reaches here, because the words of the far side come back on
/// its own standard error of the channel.
#[must_use]
pub fn of_ssh_line(line: &str) -> Option<LinkRefusal> {
    let line = line.trim();
    let names = [
        ("could not resolve hostname", LinkRefusal::Name),
        ("name or service not known", LinkRefusal::Name),
        ("temporary failure in name resolution", LinkRefusal::Name),
        ("remote host identification has changed", LinkRefusal::HostKey),
        ("host key verification failed", LinkRefusal::HostKey),
        ("permission denied (publickey", LinkRefusal::Login),
        ("permission denied, please try again", LinkRefusal::Login),
        ("connection refused", LinkRefusal::Address),
        ("connection timed out", LinkRefusal::Address),
        ("operation timed out", LinkRefusal::Address),
        ("connection reset by peer", LinkRefusal::Address),
        ("network is unreachable", LinkRefusal::Address),
        ("no route to host", LinkRefusal::Address),
        ("broken pipe", LinkRefusal::Address),
        ("command not found", LinkRefusal::Command),
        ("not found", LinkRefusal::Command),
        ("no such file or directory", LinkRefusal::Command),
    ];
    let lower = line.to_ascii_lowercase();
    names
        .into_iter()
        .find(|(phrase, _)| lower.contains(phrase))
        .map(|(_, refusal)| refusal)
}

/// The reason that the exit status of the child names, when no line named one.
#[must_use]
pub fn of_exit_code(code: i32) -> Option<LinkRefusal> {
    match code {
        // The shell of the far side ran the command of the link and did not find
        // it: the program of the server is not where the settings say.
        127 => Some(LinkRefusal::Command),
        // `ssh` reports a failure of the connection itself with its own status,
        // and the server program that did not answer has its own reason already.
        SSH_CONNECTION_FAILURE => Some(LinkRefusal::Other),
        _ => None,
    }
}

/// The reason of one attempt, written by the thread that reads the words of the
/// child and by the loop of the link. The first reason that an attempt leaves
/// stands, so a later line cannot hide the one that names the failure.
#[derive(Clone, Debug, Default)]
pub(crate) struct RefusalCell(Arc<Mutex<Option<LinkRefusal>>>);

impl RefusalCell {
    /// Keeps `refusal` when this attempt has no reason yet.
    pub(crate) fn set(&self, refusal: LinkRefusal) {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.is_none() {
            *slot = Some(refusal);
        }
    }

    /// The reason that this attempt left, when one of its lines named one.
    pub(crate) fn get(&self) -> Option<LinkRefusal> {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// How long the end of an attempt waits for the thread that reads the words of the
/// child, so the reason of that attempt is complete before a person reads it.
pub(crate) const STDERR_DRAIN_BOUND: Duration = Duration::from_millis(500);

/// Waits for the reader of the words of a child, within [`STDERR_DRAIN_BOUND`].
pub(crate) fn drain_within(finished: impl Fn() -> bool) {
    let deadline = Instant::now() + STDERR_DRAIN_BOUND;
    while !finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::{LinkRefusal, of_exit_code, of_ssh_line};

    #[test]
    fn the_words_of_ssh_name_the_reason_of_the_attempt() {
        for (line, expected) in [
            (
                "ssh: Could not resolve hostname vps.example: Name or service not known",
                LinkRefusal::Name,
            ),
            (
                "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\nWARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!",
                LinkRefusal::HostKey,
            ),
            ("Host key verification failed.", LinkRefusal::HostKey),
            (
                "deploy@203.0.113.10: Permission denied (publickey).",
                LinkRefusal::Login,
            ),
            (
                "ssh: connect to host 203.0.113.10 port 22: Connection refused",
                LinkRefusal::Address,
            ),
            (
                "ssh: connect to host 203.0.113.10 port 22: Connection timed out",
                LinkRefusal::Address,
            ),
            (
                "ssh: connect to host 10.0.0.9 port 22: No route to host",
                LinkRefusal::Address,
            ),
            ("Connection reset by peer", LinkRefusal::Address),
            ("sh: 1: /opt/horizon: not found", LinkRefusal::Command),
        ] {
            assert_eq!(of_ssh_line(line), Some(expected), "{line}");
        }
        assert_eq!(
            of_ssh_line("Warning: Permanently added '203.0.113.10' to the list of known hosts."),
            None,
            "a line that names no failure leaves the attempt without a reason"
        );
    }

    #[test]
    fn the_exit_status_of_the_child_names_a_reason_of_its_own() {
        assert_eq!(
            of_exit_code(127),
            Some(LinkRefusal::Command),
            "the command of the link is gone"
        );
        assert_eq!(of_exit_code(255), Some(LinkRefusal::Other), "ssh's own failure");
        assert_eq!(of_exit_code(0), None);
        assert_eq!(of_exit_code(1), None);
    }

    /// B15: the reason that a person reads is one fixed sentence, so no address of
    /// a server, no name of a computer and no part of a secret can travel with it.
    #[test]
    fn every_sentence_carries_no_address_and_no_name() {
        for refusal in [
            LinkRefusal::Address,
            LinkRefusal::Name,
            LinkRefusal::HostKey,
            LinkRefusal::Login,
            LinkRefusal::Command,
            LinkRefusal::Silent,
            LinkRefusal::Other,
        ] {
            let sentence = refusal.sentence();
            assert!(!sentence.is_empty(), "{}", refusal.code());
            assert!(
                !sentence.contains('@') && !sentence.contains("://") && !sentence.chars().any(|c| c.is_ascii_digit()),
                "{} names no address or port: {sentence}",
                refusal.code()
            );
        }
    }
}
