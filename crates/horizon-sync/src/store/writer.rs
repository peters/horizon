//! The one writer task of the server store.
//!
//! One thread owns the write connection. A push is one `BEGIN IMMEDIATE`
//! transaction with no network call inside it, and the answer is sent after the
//! commit, so a caller that got an answer holds a change that is on the disk. The
//! thread also publishes the new index to the readers that wait for a change, and
//! it checkpoints the write-ahead log when it ends, so a copy of the file alone is
//! the whole store.

use std::{path::Path, sync::mpsc};

use rusqlite::{Connection, Transaction, TransactionBehavior};

use crate::error::StoreError;
use crate::intent::{Intent, PushReply};
use crate::store::link::{self, JoinAnswer};
use crate::store::secrets::SecretKey;
use crate::store::{Signal, apply, now_millis, schema};

/// One job of the writer.
pub(super) enum Job {
    /// Applies the intents of one computer, and answers with their results.
    Push {
        device: String,
        intents: Vec<Intent>,
        reply: mpsc::SyncSender<Result<PushReply, StoreError>>,
    },
    /// Writes one invitation of the hash of one code.
    Invite {
        code_hash: String,
        now_millis: i64,
        reply: mpsc::SyncSender<Result<i64, StoreError>>,
    },
    /// Uses one invitation for one device, with the hash of its token.
    Join {
        code_hash: String,
        device: String,
        token_hash: String,
        now_millis: i64,
        reply: mpsc::SyncSender<Result<JoinAnswer, StoreError>>,
    },
    /// Writes the row of one device at its greeting.
    TouchDevice {
        device: String,
        login: String,
        now_millis: i64,
        reply: mpsc::SyncSender<Result<(), StoreError>>,
    },
    /// Checks the token of a device, and stamps the time that the server saw it.
    PresentToken {
        token_hash: String,
        device: String,
        now_millis: i64,
        reply: mpsc::SyncSender<Result<link::TokenLogin, StoreError>>,
    },
    /// Forgets the token and the row of one device, and keeps the record of its
    /// removal.
    RemoveDevice {
        device: String,
        now_millis: i64,
        reply: mpsc::SyncSender<Result<bool, StoreError>>,
    },
    /// Deletes every open invitation.
    ClearInvites {
        reply: mpsc::SyncSender<Result<(), StoreError>>,
    },
    /// Drops every row that a join wrote and that no hello reached in time.
    PruneUnjoined {
        now_millis: i64,
        reply: mpsc::SyncSender<Result<usize, StoreError>>,
    },
    /// Ends the writer, so the store file is whole when this call returns.
    Stop(mpsc::SyncSender<()>),
}

/// Runs the writer until the channel closes or a [`Job::Stop`] arrives.
pub(super) fn run(path: &Path, signal: &Signal, key: &SecretKey, jobs: &mpsc::Receiver<Job>) {
    let mut connection = match schema::open(path) {
        Ok(connection) => connection,
        Err(error) => {
            tracing::error!(%error, "the writer of the server store could not open its file");
            return;
        }
    };
    while let Ok(job) = jobs.recv() {
        match job {
            Job::Push { device, intents, reply } => {
                let result = write(&mut connection, &device, &intents, key, signal);
                if reply.send(result).is_err() {
                    tracing::debug!("the caller of a push stopped before the answer");
                }
            }
            Job::Invite {
                code_hash,
                now_millis,
                reply,
            } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::invite(transaction, &code_hash, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::Join {
                code_hash,
                device,
                token_hash,
                now_millis,
                reply,
            } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::join(transaction, &code_hash, &device, &token_hash, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::TouchDevice {
                device,
                login,
                now_millis,
                reply,
            } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::touch(transaction, &device, &login, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::PresentToken {
                token_hash,
                device,
                now_millis,
                reply,
            } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::present(transaction, &token_hash, &device, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::RemoveDevice {
                device,
                now_millis,
                reply,
            } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::remove(transaction, &device, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::ClearInvites { reply } => {
                let result = in_transaction(&mut connection, link::clear_invites);
                let _ = reply.send(result);
            }
            Job::PruneUnjoined { now_millis, reply } => {
                let result = in_transaction(&mut connection, |transaction| {
                    link::prune_unjoined(transaction, now_millis)
                });
                let _ = reply.send(result);
            }
            Job::Stop(reply) => {
                schema::checkpoint(&connection);
                drop(connection);
                if reply.send(()).is_err() {
                    tracing::debug!("the caller that stopped the writer is gone");
                }
                return;
            }
        }
    }
    schema::checkpoint(&connection);
}

/// Runs `write` as one immediate transaction, and commits it when it answered.
fn in_transaction<T>(
    connection: &mut Connection,
    write: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
) -> Result<T, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let answer = write(&transaction)?;
    transaction.commit()?;
    Ok(answer)
}

/// Applies one push in one transaction, commits it, and then tells the readers.
fn write(
    connection: &mut rusqlite::Connection,
    device: &str,
    intents: &[Intent],
    key: &SecretKey,
    signal: &Signal,
) -> Result<PushReply, StoreError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let reply = apply::push(&transaction, device, intents, key, now_millis())?;
    let index = schema::index_of(&transaction)?;
    transaction.commit()?;
    signal.publish(index);
    Ok(reply)
}
