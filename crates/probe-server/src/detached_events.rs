use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use probe_protocol::runtime::{
    DetachedSessionEventPayload, DetachedSessionEventRecord, DetachedSessionEventTruth,
};
use probe_protocol::session::{SessionId, TimestampMs};

#[derive(Debug)]
pub(crate) enum DetachedEventError {
    Io(io::Error),
    Json(serde_json::Error),
}

impl std::fmt::Display for DetachedEventError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Json(error) => write!(f, "json error: {error}"),
        }
    }
}

impl std::error::Error for DetachedEventError {}

impl From<io::Error> for DetachedEventError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DetachedEventError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone)]
pub(crate) struct DetachedSessionEventHub {
    events_root: PathBuf,
    append_lock: Arc<Mutex<()>>,
    next_cursor: Arc<Mutex<HashMap<String, u64>>>,
    subscribers: Arc<Mutex<HashMap<String, Vec<Sender<DetachedSessionEventRecord>>>>>,
}

impl DetachedSessionEventHub {
    pub(crate) fn new(probe_home: &Path) -> Self {
        Self {
            events_root: probe_home.join("daemon").join("events"),
            append_lock: Arc::new(Mutex::new(())),
            next_cursor: Arc::new(Mutex::new(HashMap::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn append(
        &self,
        session_id: &SessionId,
        truth: DetachedSessionEventTruth,
        payload: DetachedSessionEventPayload,
        timestamp_ms: TimestampMs,
    ) -> Result<DetachedSessionEventRecord, DetachedEventError> {
        let _append_lock = self
            .append_lock
            .lock()
            .expect("detached session append mutex should not be poisoned");
        let cursor = self.next_cursor_for(session_id)?;
        let record = DetachedSessionEventRecord {
            cursor,
            session_id: session_id.clone(),
            timestamp_ms,
            truth,
            payload,
        };
        self.write_record(&record)?;
        self.publish(record.clone());
        Ok(record)
    }

    pub(crate) fn read(
        &self,
        session_id: &SessionId,
        after_cursor: Option<u64>,
        limit: usize,
    ) -> Result<Vec<DetachedSessionEventRecord>, DetachedEventError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let path = self.session_events_path(session_id);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(path)?;
        if let Some(after_cursor) = after_cursor {
            let mut events = Vec::new();
            for line in BufReader::new(file).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                let record: DetachedSessionEventRecord = serde_json::from_str(&line)?;
                if record.cursor <= after_cursor {
                    continue;
                }
                events.push(record);
                if events.len() >= limit {
                    break;
                }
            }
            return Ok(events);
        }
        let mut tail = VecDeque::with_capacity(limit);
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: DetachedSessionEventRecord = serde_json::from_str(&line)?;
            if tail.len() == limit {
                tail.pop_front();
            }
            tail.push_back(record);
        }
        Ok(tail.into_iter().collect())
    }

    pub(crate) fn newest_cursor(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<u64>, DetachedEventError> {
        let path = self.session_events_path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let file = File::open(path)?;
        let mut newest = None;
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let record: DetachedSessionEventRecord = serde_json::from_str(&line)?;
            newest = Some(record.cursor);
        }
        Ok(newest)
    }

    pub(crate) fn subscribe(&self, session_id: &SessionId) -> Receiver<DetachedSessionEventRecord> {
        let (sender, receiver) = mpsc::channel();
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("detached session event subscribers mutex should not be poisoned");
        subscribers
            .entry(String::from(session_id.as_str()))
            .or_default()
            .push(sender);
        receiver
    }

    fn next_cursor_for(&self, session_id: &SessionId) -> Result<u64, DetachedEventError> {
        let mut next_cursor = self
            .next_cursor
            .lock()
            .expect("detached session event cursor mutex should not be poisoned");
        let entry = next_cursor.entry(String::from(session_id.as_str()));
        let cursor = match entry {
            std::collections::hash_map::Entry::Occupied(mut entry) => {
                let cursor = *entry.get();
                *entry.get_mut() += 1;
                cursor
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let next = self
                    .newest_cursor(session_id)?
                    .map(|cursor| cursor + 1)
                    .unwrap_or(0);
                entry.insert(next + 1);
                next
            }
        };
        Ok(cursor)
    }

    fn write_record(&self, record: &DetachedSessionEventRecord) -> Result<(), DetachedEventError> {
        if let Some(parent) = self.session_events_path(&record.session_id).parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.session_events_path(&record.session_id))?;
        serde_json::to_writer(&mut file, record)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    fn publish(&self, record: DetachedSessionEventRecord) {
        let mut subscribers = self
            .subscribers
            .lock()
            .expect("detached session event subscribers mutex should not be poisoned");
        if let Some(entries) = subscribers.get_mut(record.session_id.as_str()) {
            entries.retain(|sender| sender.send(record.clone()).is_ok());
        }
    }

    fn session_events_path(&self, session_id: &SessionId) -> PathBuf {
        self.events_root
            .join(format!("{}.jsonl", session_id.as_str()))
    }
}
