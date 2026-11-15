//! Imports token usage from the local Codex and Claude session stores.
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use tokio_util::sync::CancellationToken;

use crate::{
    model::LocalAgentUsageRecord,
    store::{LocalAgentHistoryStatus, Store},
    Error, Result,
};

mod claude;
mod codex;

const SCAN_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
}

#[derive(Default)]
struct Scanner {
    stamps: HashMap<PathBuf, FileStamp>,
}

struct ScanBatch {
    records: Vec<LocalAgentUsageRecord>,
    stamps: HashMap<PathBuf, FileStamp>,
    errors: Vec<String>,
}

pub async fn run(store: Store, shutdown: CancellationToken) {
    let mut scanner = Scanner::default();
    loop {
        match scanner.sync(&store).await {
            Ok(status) => tracing::info!(
                events = status.event_count,
                "local agent history scan completed"
            ),
            Err(error) => tracing::warn!(%error, "local agent history scan failed"),
        }
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = tokio::time::sleep(SCAN_INTERVAL) => {}
        }
    }
}

pub async fn sync(store: &Store) -> Result<LocalAgentHistoryStatus> {
    Scanner::default().sync(store).await
}

impl Scanner {
    async fn sync(&mut self, store: &Store) -> Result<LocalAgentHistoryStatus> {
        let previous = std::mem::take(&mut self.stamps);
        let roots = ScanRoots::current();
        let batch = tokio::task::spawn_blocking(move || scan(roots, previous))
            .await
            .map_err(|error| {
                Error::Store(format!("local agent history scan task failed: {error}"))
            })?;
        self.stamps = batch.stamps;
        let scan_error = (!batch.errors.is_empty()).then(|| batch.errors.join("; "));
        let result = store.upsert_local_agent_usage(&batch.records).await;
        match result {
            Ok(_) => {
                store.record_local_agent_usage_sync(scan_error).await?;
                store.local_agent_history_status().await
            }
            Err(error) => {
                store
                    .record_local_agent_usage_sync(Some(error.to_string()))
                    .await?;
                Err(error)
            }
        }
    }
}

struct ScanRoots {
    codex: Option<PathBuf>,
    claude_code: Option<PathBuf>,
    claude_cli: Option<PathBuf>,
}

impl ScanRoots {
    fn current() -> Self {
        let home = dirs::home_dir();
        Self {
            codex: home.as_ref().map(|home| home.join(".codex/sessions")),
            claude_code: home.as_ref().map(|home| home.join(".claude/projects")),
            claude_cli: dirs::data_dir()
                .map(|data| data.join("Claude-3p/local-agent-mode-sessions")),
        }
    }
}

fn scan(roots: ScanRoots, previous: HashMap<PathBuf, FileStamp>) -> ScanBatch {
    let mut batch = ScanBatch {
        records: Vec::new(),
        stamps: HashMap::new(),
        errors: Vec::new(),
    };
    let mut records = BTreeMap::new();
    if let Some(root) = roots.codex {
        scan_root(&root, &previous, &mut batch, |path, stamps, errors| {
            record_result(codex::scan_file(path), path, stamps, errors)
        });
    }
    if let Some(root) = roots.claude_code {
        scan_root(&root, &previous, &mut batch, |path, stamps, errors| {
            record_result(claude::scan_code_projects(path), path, stamps, errors)
        });
    }
    if let Some(root) = roots.claude_cli {
        scan_root(&root, &previous, &mut batch, |path, stamps, errors| {
            record_result(claude::scan_usage_ledger(path), path, stamps, errors)
        });
    }
    for record in batch.records.drain(..) {
        records.insert(record.event_key.clone(), record);
    }
    batch.records = records.into_values().collect();
    batch
}

fn scan_root(
    root: &Path,
    previous: &HashMap<PathBuf, FileStamp>,
    batch: &mut ScanBatch,
    mut parse: impl FnMut(
        &Path,
        &mut HashMap<PathBuf, FileStamp>,
        &mut Vec<String>,
    ) -> Vec<LocalAgentUsageRecord>,
) {
    let mut files = Vec::new();
    collect_files(root, &mut files, &mut batch.errors);
    for path in files {
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                batch.errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let stamp = FileStamp {
            len: metadata.len(),
            modified: metadata.modified().ok(),
        };
        if previous.get(&path) == Some(&stamp) {
            batch.stamps.insert(path, stamp);
            continue;
        }
        let records = parse(&path, &mut batch.stamps, &mut batch.errors);
        batch.records.extend(records);
    }
}

fn record_result(
    result: std::result::Result<Vec<LocalAgentUsageRecord>, String>,
    path: &Path,
    stamps: &mut HashMap<PathBuf, FileStamp>,
    errors: &mut Vec<String>,
) -> Vec<LocalAgentUsageRecord> {
    match result {
        Ok(records) => {
            if let Ok(metadata) = fs::metadata(path) {
                stamps.insert(
                    path.to_owned(),
                    FileStamp {
                        len: metadata.len(),
                        modified: metadata.modified().ok(),
                    },
                );
            }
            records
        }
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    }
}

fn collect_files(path: &Path, files: &mut Vec<PathBuf>, errors: &mut Vec<String>) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            errors.push(format!("{}: {error}", path.display()));
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                errors.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if file_type.is_dir() {
            collect_files(&path, files, errors);
        } else if file_type.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("jsonl" | "ndjson")
            )
        {
            files.push(path);
        }
    }
}
