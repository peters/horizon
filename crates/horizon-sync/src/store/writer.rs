//! The one writer task of the server store.
//!
//! One thread owns the write connection. A push is one `BEGIN IMMEDIATE`
//! transaction with no network call inside it, and the answer is sent after the
//! commit, so a caller that got an answer holds a change that is on the disk. The
//! thread also publishes the new index to the readers that wait for a change, and
//! it checkpoints the write-ahead log when it ends, so a copy of the file alone is
//! the whole store.

use std::{path::Path, sync::mpsc};

use rusqlite::TransactionBehavior;

use crate::error::StoreError;
use crate::intent::{Intent, PushReply};
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
