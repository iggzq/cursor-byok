//! Cross-machine overview aggregation.
//!
//! `server` is the standalone collector service that runs on a central host;
//! `worker` pushes this machine's records to it; `remote` reads aggregates back.
mod config;
mod remote;
mod server;
mod worker;

pub use config::CollectorConfig;
pub use remote::{RemoteClient, RemoteQuery};
pub use server::{log_binding, report_bind_error, router};
pub use worker::{flush, reset_progress, run};

/// Compares two secrets without leaking their content through timing.
pub(crate) fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let difference = left
        .iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        });
    difference == 0
}
