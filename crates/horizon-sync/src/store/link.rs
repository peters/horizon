//! The login of a computer: the invitations of a server, the token of each device
//! and the list of the computers.
//!
//! A person who is logged in makes an invitation, which is one code of 12
//! characters for one use in 10 minutes. The server keeps the SHA-256 of the code
//! and never the code. A computer that presents a code gets one token of 32 bytes,
//! and the server keeps the SHA-256 of that token as the login of one device, so a
//! computer can never be asked for its token again.
//!
//! The wrong-code guard is one object of the running server: 20 wrong codes inside
//! 10 minutes delete every open invitation, and `join` then answers `join_locked`
//! until a person makes a new invitation. It is not a table, because it protects
//! the invitations of this process and a restart of the server begins a new window.

use std::collections::VecDeque;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::error::StoreError;
use crate::protocol::{DeviceLogin, DeviceRow};

/// How long an invitation lives.
pub const INVITE_LIFETIME_MILLIS: i64 = 600_000;

/// The most invitations that a server keeps open. A fifth replaces the oldest.
pub const MAX_OPEN_INVITES: usize = 4;

/// The most wrong codes inside [`WRONG_CODE_WINDOW_MILLIS`] before every open
/// invitation is deleted.
pub const MAX_WRONG_CODES: usize = 20;

/// The window of the wrong codes, in milliseconds.
pub const WRONG_CODE_WINDOW_MILLIS: i64 = 600_000;

/// What the server did with the code of a join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinAnswer {
    /// The code was one open invitation, and the token is now the login of that
    /// device.
    Joined,
    /// The code is not one open invitation of this server: unknown, expired or
    /// used. The three answers are one, so the answer is no oracle.
    WrongCode,
    /// Too many wrong codes were tried, and every invitation is gone.
    Locked,
    /// The device name already holds a token, so the person removes it first.
    NameTaken,
}

/// The wrong codes of the running server, and whether they closed the door.
#[derive(Debug, Default)]
pub(super) struct WrongCodes {
    times: VecDeque<i64>,
    locked: bool,
}

impl WrongCodes {
    /// Whether a join now answers `join_locked`.
    pub(super) fn is_locked(&self) -> bool {
        self.locked
    }

    /// Records one wrong code at `now_millis` and answers whether the door is
    /// closed now. A code older than the window no longer counts.
    pub(super) fn record(&mut self, now_millis: i64) -> bool {
        let oldest = now_millis.saturating_sub(WRONG_CODE_WINDOW_MILLIS);
        while self.times.front().is_some_and(|time| *time <= oldest) {
            self.times.pop_front();
        }
        self.times.push_back(now_millis);
        if self.times.len() >= MAX_WRONG_CODES {
            self.locked = true;
        }
        self.locked
    }

    /// Opens the door again, which one new invitation does.
    pub(super) fn clear(&mut self) {
        self.times.clear();
        self.locked = false;
    }
}

/// Writes one invitation of `code_hash` and answers the time of the server when it
/// ends. Expired invitations are removed first, and the oldest open one makes room
/// for a fifth.
///
/// # Errors
/// Returns why the invitation cannot be written.
pub(super) fn invite(transaction: &Transaction<'_>, code_hash: &str, now_millis: i64) -> Result<i64, StoreError> {
    transaction.execute("DELETE FROM invitations WHERE expires_millis <= ?1", [now_millis])?;
    let expires_millis = now_millis.saturating_add(INVITE_LIFETIME_MILLIS);
    transaction.execute(
        "INSERT INTO invitations (code_hash, created_millis, expires_millis) VALUES (?1, ?2, ?3)
         ON CONFLICT (code_hash) DO UPDATE SET created_millis = excluded.created_millis,
             expires_millis = excluded.expires_millis",
        params![code_hash, now_millis, expires_millis],
    )?;
    transaction.execute(
        "DELETE FROM invitations WHERE code_hash NOT IN (
             SELECT code_hash FROM invitations ORDER BY created_millis DESC, rowid DESC LIMIT ?1
         )",
        [i64::try_from(MAX_OPEN_INVITES).unwrap_or(4)],
    )?;
    Ok(expires_millis)
}

/// Deletes every open invitation, which the wrong-code guard does.
///
/// # Errors
/// Returns why the invitations cannot be removed.
pub(super) fn clear_invites(transaction: &Transaction<'_>) -> Result<(), StoreError> {
    transaction.execute("DELETE FROM invitations", [])?;
    Ok(())
}

/// Uses the invitation of `code_hash` for `device` and stores the hash of its
/// token.
///
/// # Errors
/// Returns why the invitation cannot be used.
pub(super) fn join(
    transaction: &Transaction<'_>,
    code_hash: &str,
    device: &str,
    token_hash: &str,
    now_millis: i64,
) -> Result<JoinAnswer, StoreError> {
    let expires: Option<i64> = transaction
        .query_row(
            "SELECT expires_millis FROM invitations WHERE code_hash = ?1",
            [code_hash],
            |row| row.get(0),
        )
        .optional()?;
    if expires.is_none_or(|ends| ends <= now_millis) {
        // An expired invitation is one wrong code, and it is gone.
        transaction.execute("DELETE FROM invitations WHERE code_hash = ?1", [code_hash])?;
        return Ok(JoinAnswer::WrongCode);
    }
    let taken: Option<i64> = transaction
        .query_row(
            "SELECT 1 FROM devices WHERE name = ?1 AND token_hash IS NOT NULL",
            [device],
            |row| row.get(0),
        )
        .optional()?;
    if taken.is_some() {
        // The code stays open, because the person removes the old entry and uses
        // the same code again.
        return Ok(JoinAnswer::NameTaken);
    }
    transaction.execute("DELETE FROM invitations WHERE code_hash = ?1", [code_hash])?;
    transaction.execute(
        "INSERT INTO devices (name, first_seen, last_seen, last_intent, token_hash, created_millis, login)
         VALUES (?1, ?2, ?2, 0, ?3, ?2, 'token')
         ON CONFLICT (name) DO UPDATE SET token_hash = excluded.token_hash,
             last_seen = excluded.last_seen, created_millis = excluded.created_millis, login = 'token'",
        params![device, now_millis, token_hash],
    )?;
    Ok(JoinAnswer::Joined)
}

/// Writes the row of `device` and the time of this greeting, so the list of the
/// computers shows a computer that a person reaches over `ssh` and that never sent
/// an intent.
///
/// A row that joined with a token keeps that login: a computer does not become an
/// `ssh` computer because it also reached the socket.
///
/// # Errors
/// Returns why the row cannot be written.
pub(super) fn touch(
    transaction: &Transaction<'_>,
    device: &str,
    login: &str,
    now_millis: i64,
) -> Result<(), StoreError> {
    transaction.execute(
        "INSERT INTO devices (name, first_seen, last_seen, last_intent, login) VALUES (?1, ?2, ?2, 0, ?3)
         ON CONFLICT (name) DO UPDATE SET last_seen = excluded.last_seen,
             login = CASE WHEN devices.login = 'token' THEN 'token' ELSE excluded.login END",
        params![device, now_millis, login],
    )?;
    Ok(())
}

/// Whether `token_hash` is the token of `device`, and when the server saw that
/// device last.
///
/// # Errors
/// Returns why the row cannot be read or written.
pub(super) fn present(
    transaction: &Transaction<'_>,
    token_hash: &str,
    device: &str,
    now_millis: i64,
) -> Result<bool, StoreError> {
    let name: Option<String> = transaction
        .query_row("SELECT name FROM devices WHERE token_hash = ?1", [token_hash], |row| {
            row.get(0)
        })
        .optional()?;
    if name.as_deref() != Some(device) {
        return Ok(false);
    }
    transaction.execute(
        "UPDATE devices SET last_seen = ?2 WHERE name = ?1",
        params![device, now_millis],
    )?;
    Ok(true)
}

/// Forgets the token and the row of `device`, and answers whether the server held
/// it.
///
/// # Errors
/// Returns why the row cannot be removed.
pub(super) fn remove(transaction: &Transaction<'_>, device: &str) -> Result<bool, StoreError> {
    let removed = transaction.execute("DELETE FROM devices WHERE name = ?1", [device])?;
    Ok(removed > 0)
}

/// The computers of this server, in name order, with the row of `caller` marked.
///
/// # Errors
/// Returns why the rows cannot be read.
pub(super) fn devices(connection: &Connection, caller: &str) -> Result<Vec<DeviceRow>, StoreError> {
    let mut statement = connection
        .prepare_cached("SELECT name, first_seen, last_seen, created_millis, login FROM devices ORDER BY name")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(name, first_seen, last_seen, created, login)| {
            Ok(DeviceRow {
                caller: name == caller,
                name,
                created_millis: non_negative(if created > 0 { created } else { first_seen })?,
                last_seen_millis: non_negative(last_seen)?,
                login: if login == "token" {
                    DeviceLogin::Token
                } else {
                    DeviceLogin::Ssh
                },
            })
        })
        .collect()
}

fn non_negative(value: i64) -> Result<u64, StoreError> {
    u64::try_from(value).map_err(|_| StoreError::Damaged(format!("the time {value} is not one")))
}

/// The number of open invitations, for a test and for a log line.
///
/// # Errors
/// Returns why the rows cannot be read.
pub(super) fn open_invites(connection: &Connection, now_millis: i64) -> Result<usize, StoreError> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM invitations WHERE expires_millis > ?1",
        [now_millis],
        |row| row.get(0),
    )?;
    usize::try_from(count).map_err(|_| StoreError::Damaged(format!("the count {count} is not one")))
}
