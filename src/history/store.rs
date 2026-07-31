//! Durable publication and append boundary for canonical history-v1.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use super::codec::{
    decode_record, encode_record, BootRecord, Capacity, CheckpointRecord, DecodeError, Record,
    SampleRecord, StateEntry,
};

const CANONICAL_NAME: &str = "history-v1.jsonl";
const TEMP_PREFIX: &str = "history-v1.jsonl.tmp-";

#[derive(Debug)]
pub struct HistoryStore {
    file: File,
    boot_id: String,
    last_state: Option<Vec<StateEntry>>,
    replay_records: Vec<Record>,
    last_timestamp: Option<u64>,
    poisoned: bool,
}

#[derive(Debug)]
pub enum OpenError {
    Io(io::Error),
    EmptyCanonical,
    InvalidCanonical { line: usize, error: DecodeError },
    UnterminatedRecord,
    InvalidStream { line: usize, reason: &'static str },
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "history store I/O error: {error}"),
            Self::EmptyCanonical => formatter.write_str("canonical history is empty"),
            Self::InvalidCanonical { line, error } => write!(
                formatter,
                "invalid canonical history at line {line}: {}",
                error.diagnostic()
            ),
            Self::UnterminatedRecord => {
                formatter.write_str("canonical history ends with an unterminated record")
            }
            Self::InvalidStream { line, reason } => {
                write!(
                    formatter,
                    "invalid canonical history stream at line {line}: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for OpenError {}

impl From<io::Error> for OpenError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<WriteError> for OpenError {
    fn from(error: WriteError) -> Self {
        match error {
            WriteError::Io(error) => Self::Io(error),
            WriteError::Encode => Self::Io(io::Error::other("canonical history encode failure")),
            WriteError::Poisoned => {
                Self::Io(io::Error::other("canonical history store is poisoned"))
            }
            WriteError::TimestampRegression => {
                Self::Io(io::Error::other("canonical history timestamp regresses"))
            }
        }
    }
}

#[derive(Debug)]
pub enum WriteError {
    Encode,
    Io(io::Error),
    Poisoned,
    TimestampRegression,
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Encode => formatter.write_str("cannot encode canonical history record"),
            Self::Io(error) => write!(formatter, "history store I/O error: {error}"),
            Self::Poisoned => {
                formatter.write_str("canonical history store is poisoned after a failed append")
            }
            Self::TimestampRegression => {
                formatter.write_str("canonical history timestamp regresses")
            }
        }
    }
}

impl std::error::Error for WriteError {}

impl HistoryStore {
    pub fn open(data_dir: &Path, timestamp: u64, capacity: Capacity) -> Result<Self, OpenError> {
        Self::open_inner(data_dir, timestamp, capacity, None)
    }

    fn open_inner(
        data_dir: &Path,
        timestamp: u64,
        capacity: Capacity,
        mut failure: Option<&mut FailureSwitch>,
    ) -> Result<Self, OpenError> {
        let path = data_dir.join(CANONICAL_NAME);
        let boot_id = new_boot_id();
        let boot = Record::Boot(BootRecord {
            timestamp,
            boot_id: boot_id.clone(),
            capacity,
        });
        match OpenOptions::new().read(true).open(&path) {
            Ok(_) => {
                let mut records = validate_canonical(&path)?;
                ensure_open_timestamp(&records, &boot)?;
                let mut file = OpenOptions::new().append(true).open(&path)?;
                append_record(&mut file, &boot, failure.as_deref_mut(), true)?;
                records.push(boot);
                let last_timestamp = records.last().map(record_timestamp);
                Ok(Self {
                    file,
                    boot_id,
                    last_state: None,
                    replay_records: records,
                    last_timestamp,
                    poisoned: false,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir_all(data_dir)?;
                match publish_first_boot(&path, &boot, failure.as_deref_mut()) {
                    Ok(()) => {
                        let file = OpenOptions::new().append(true).open(&path)?;
                        let last_timestamp = Some(record_timestamp(&boot));
                        Ok(Self {
                            file,
                            boot_id,
                            last_state: None,
                            replay_records: vec![boot],
                            last_timestamp,
                            poisoned: false,
                        })
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                        let mut records = validate_canonical(&path)?;
                        ensure_open_timestamp(&records, &boot)?;
                        let mut file = OpenOptions::new().append(true).open(&path)?;
                        append_record(&mut file, &boot, failure, true)?;
                        records.push(boot);
                        let last_timestamp = records.last().map(record_timestamp);
                        Ok(Self {
                            file,
                            boot_id,
                            last_state: None,
                            replay_records: records,
                            last_timestamp,
                            poisoned: false,
                        })
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    pub fn append_sample(
        &mut self,
        timestamp: u64,
        capacity: Capacity,
        state: Vec<StateEntry>,
    ) -> Result<(), WriteError> {
        self.append_sample_inner(timestamp, capacity, state, None)
    }

    fn append_sample_inner(
        &mut self,
        timestamp: u64,
        capacity: Capacity,
        state: Vec<StateEntry>,
        failure: Option<&mut FailureSwitch>,
    ) -> Result<(), WriteError> {
        if self.poisoned {
            return Err(WriteError::Poisoned);
        }
        if self.last_state.as_ref() == Some(&state) {
            return self.checkpoint(timestamp, capacity);
        }
        let record = Record::Sample(SampleRecord {
            timestamp,
            boot_id: self.boot_id.clone(),
            capacity,
            state: state.clone(),
        });
        ensure_runtime_timestamp(self.last_timestamp, &record)?;
        if let Err(error) = append_record(&mut self.file, &record, failure, false) {
            self.poisoned = true;
            return Err(error);
        }
        self.last_timestamp = Some(record_timestamp(&record));
        self.last_state = Some(state);
        Ok(())
    }

    pub fn checkpoint(&mut self, timestamp: u64, capacity: Capacity) -> Result<(), WriteError> {
        self.checkpoint_inner(timestamp, capacity, None)
    }

    fn checkpoint_inner(
        &mut self,
        timestamp: u64,
        capacity: Capacity,
        failure: Option<&mut FailureSwitch>,
    ) -> Result<(), WriteError> {
        if self.poisoned {
            return Err(WriteError::Poisoned);
        }
        let record = Record::Checkpoint(CheckpointRecord {
            timestamp,
            boot_id: self.boot_id.clone(),
            capacity,
        });
        ensure_runtime_timestamp(self.last_timestamp, &record)?;
        if let Err(error) = append_record(&mut self.file, &record, failure, false) {
            self.poisoned = true;
            return Err(error);
        }
        self.last_timestamp = Some(record_timestamp(&record));
        Ok(())
    }

    pub(crate) fn boot_id(&self) -> &str {
        &self.boot_id
    }

    pub(crate) fn take_replay_records(&mut self) -> Vec<Record> {
        std::mem::take(&mut self.replay_records)
    }

    pub(crate) fn file_bytes(&self) -> io::Result<u64> {
        self.file.metadata().map(|metadata| metadata.len())
    }
}

pub(crate) fn stale_temporary_count(directory: &Path) -> io::Result<usize> {
    fs::read_dir(directory)?.try_fold(0, |count, entry| {
        let name = entry?.file_name();
        Ok(count + usize::from(name.to_string_lossy().starts_with(TEMP_PREFIX)))
    })
}

fn validate_canonical(path: &Path) -> Result<Vec<Record>, OpenError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut active_boot: Option<String> = None;
    let mut last_timestamp = None;
    let mut line = Vec::new();
    let mut index = 0;
    loop {
        line.clear();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            break;
        }
        index += 1;
        if line.last() != Some(&b'\n') {
            return Err(OpenError::UnterminatedRecord);
        }
        line.pop();
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let record = decode_record(&line)
            .map_err(|error| OpenError::InvalidCanonical { line: index, error })?;
        let (timestamp, boot_id, is_boot) = match &record {
            Record::Boot(record) => (record.timestamp, &record.boot_id, true),
            Record::Sample(record) => (record.timestamp, &record.boot_id, false),
            Record::Checkpoint(record) => (record.timestamp, &record.boot_id, false),
        };
        if last_timestamp.is_some_and(|previous| timestamp < previous) {
            return Err(OpenError::InvalidStream {
                line: index,
                reason: "timestamp regresses",
            });
        }
        if !is_boot && active_boot.as_deref() != Some(boot_id.as_str()) {
            return Err(OpenError::InvalidStream {
                line: index,
                reason: "sample or checkpoint has no preceding matching boot",
            });
        }
        if is_boot {
            active_boot = Some(boot_id.clone());
        }
        last_timestamp = Some(timestamp);
        records.push(record);
    }
    (!records.is_empty())
        .then_some(records)
        .ok_or(OpenError::EmptyCanonical)
}

fn record_timestamp(record: &Record) -> u64 {
    match record {
        Record::Boot(record) => record.timestamp,
        Record::Sample(record) => record.timestamp,
        Record::Checkpoint(record) => record.timestamp,
    }
}

fn ensure_open_timestamp(records: &[Record], next: &Record) -> Result<(), OpenError> {
    if records
        .last()
        .is_some_and(|previous| record_timestamp(next) < record_timestamp(previous))
    {
        return Err(OpenError::InvalidStream {
            line: records.len() + 1,
            reason: "timestamp regresses",
        });
    }
    Ok(())
}

fn ensure_runtime_timestamp(last_timestamp: Option<u64>, next: &Record) -> Result<(), WriteError> {
    if last_timestamp.is_some_and(|previous| record_timestamp(next) < previous) {
        return Err(WriteError::TimestampRegression);
    }
    Ok(())
}

fn publish_first_boot(
    path: &Path,
    boot: &Record,
    mut failure: Option<&mut FailureSwitch>,
) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("history path has no parent"))?;
    let (temporary, mut file) = create_temporary(directory, &mut failure)?;
    write_record(
        &mut file,
        boot,
        failure.as_deref_mut(),
        Some(FailurePoint::TempWrite),
    )
    .map_err(write_error_to_io)?;
    file.flush()?;
    fail(&mut failure, FailurePoint::TempSync)?;
    file.sync_all()?;
    fail(&mut failure, FailurePoint::HardLink)?;
    fs::hard_link(&temporary, path)?;
    fs::remove_file(&temporary)?;
    fail(&mut failure, FailurePoint::ParentDirectorySync)?;
    sync_directory(directory)
}

fn create_temporary(
    directory: &Path,
    failure: &mut Option<&mut FailureSwitch>,
) -> io::Result<(PathBuf, File)> {
    for _ in 0..32 {
        let path = directory.join(format!("{TEMP_PREFIX}{}", new_boot_id()));
        fail(failure, FailurePoint::TempCreate)?;
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unique history temporary unavailable",
    ))
}

fn append_record(
    file: &mut File,
    record: &Record,
    mut failure: Option<&mut FailureSwitch>,
    startup_append: bool,
) -> Result<(), WriteError> {
    if startup_append {
        fail(&mut failure, FailurePoint::RestartBootAppend).map_err(WriteError::Io)?;
    }
    #[cfg(test)]
    if failure
        .as_deref()
        .is_some_and(|failure| failure.point == FailurePoint::RuntimePartialAppend)
    {
        return write_partial_record(file, record, &mut failure);
    }
    write_record(file, record, failure.as_deref_mut(), None)?;
    file.flush().map_err(WriteError::Io)?;
    if startup_append {
        fail(&mut failure, FailurePoint::RestartBootSync).map_err(WriteError::Io)?;
    }
    file.sync_all().map_err(WriteError::Io)
}

#[cfg(test)]
fn write_partial_record(
    file: &mut File,
    record: &Record,
    failure: &mut Option<&mut FailureSwitch>,
) -> Result<(), WriteError> {
    let encoded = encode_record(record).map_err(|_| WriteError::Encode)?;
    let partial = encoded.len().max(1) / 2;
    file.write_all(&encoded[..partial])
        .map_err(WriteError::Io)?;
    file.flush().map_err(WriteError::Io)?;
    fail(failure, FailurePoint::RuntimePartialAppend).map_err(WriteError::Io)
}

fn write_record(
    file: &mut File,
    record: &Record,
    mut failure: Option<&mut FailureSwitch>,
    failure_point: Option<FailurePoint>,
) -> Result<(), WriteError> {
    if let Some(point) = failure_point {
        fail(&mut failure, point).map_err(WriteError::Io)?;
    }
    let encoded = encode_record(record).map_err(|_| WriteError::Encode)?;
    file.write_all(&encoded).map_err(WriteError::Io)?;
    file.write_all(b"\n").map_err(WriteError::Io)
}

fn write_error_to_io(error: WriteError) -> io::Error {
    match error {
        WriteError::Encode => io::Error::other("canonical history encode failure"),
        WriteError::Io(error) => error,
        WriteError::Poisoned => io::Error::other("canonical history store is poisoned"),
        WriteError::TimestampRegression => {
            io::Error::other("canonical history timestamp regresses")
        }
    }
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> io::Result<()> {
    File::open(directory)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> io::Result<()> {
    Ok(())
}

fn new_boot_id() -> String {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS RNG for canonical history boot ID");
    let mut id = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FailurePoint {
    TempCreate,
    TempWrite,
    TempSync,
    HardLink,
    ParentDirectorySync,
    RestartBootAppend,
    RestartBootSync,
    RuntimePartialAppend,
}

#[cfg(not(test))]
#[derive(Clone, Copy)]
enum FailurePoint {
    TempCreate,
    TempWrite,
    TempSync,
    HardLink,
    ParentDirectorySync,
    RestartBootAppend,
    RestartBootSync,
}

struct FailureSwitch {
    #[cfg(test)]
    point: FailurePoint,
    #[cfg(test)]
    fired: Option<FailurePoint>,
}

fn fail(failure: &mut Option<&mut FailureSwitch>, point: FailurePoint) -> io::Result<()> {
    #[cfg(test)]
    if let Some(failure) = failure {
        if failure.point == point {
            failure.fired = Some(point);
            return Err(io::Error::other(format!("injected {point:?} failure")));
        }
    }
    let _ = (failure, point);
    Ok(())
}

#[cfg(test)]
struct InjectedOpen {
    result: Result<HistoryStore, OpenError>,
    fired: Option<FailurePoint>,
}

#[cfg(test)]
fn open_with_test_failure(
    data_dir: &Path,
    timestamp: u64,
    capacity: Capacity,
    point: FailurePoint,
) -> InjectedOpen {
    let mut failure = FailureSwitch { point, fired: None };
    let result = HistoryStore::open_inner(data_dir, timestamp, capacity, Some(&mut failure));
    InjectedOpen {
        result,
        fired: failure.fired,
    }
}

#[cfg(test)]
struct InjectedWrite {
    result: Result<(), WriteError>,
    fired: Option<FailurePoint>,
}

#[cfg(test)]
fn append_with_test_failure(
    store: &mut HistoryStore,
    timestamp: u64,
    capacity: Capacity,
    state: Vec<StateEntry>,
    point: FailurePoint,
) -> InjectedWrite {
    let mut failure = FailureSwitch { point, fired: None };
    let result = store.append_sample_inner(timestamp, capacity, state, Some(&mut failure));
    InjectedWrite {
        result,
        fired: failure.fired,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        append_with_test_failure, open_with_test_failure, FailurePoint, HistoryStore, OpenError,
    };
    use crate::history::codec::{decode_record, Capacity, Record, StateEntry, StateKind};

    fn capacity() -> Capacity {
        Capacity {
            capacity_rpm: 80,
            enabled_keys: 2,
            key_rpms: vec![40, 40],
        }
    }

    fn state(value: f64) -> Vec<StateEntry> {
        vec![StateEntry {
            kind: StateKind::Counter,
            metric: "nimproxy_requests_total".into(),
            labels: Default::default(),
            value,
        }]
    }

    fn test_dir(label: &str) -> std::path::PathBuf {
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "nim-proxy-history-store-{label}-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_open_publishes_one_synced_canonical_boot_record() {
        // This catches a missing safe-publication boundary, including an
        // accidental fallback to the legacy history.jsonl path.
        let dir = std::env::temp_dir().join(format!(
            "nim-proxy-history-store-red-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let result = HistoryStore::open(&dir, 1_000, capacity());
        assert!(
            result.is_ok(),
            "missing canonical path must publish a boot: {result:?}"
        );

        let bytes = fs::read(dir.join("history-v1.jsonl")).unwrap();
        let Record::Boot(boot) = decode_record(bytes.trim_ascii_end()).unwrap() else {
            panic!("publication must contain a canonical boot record");
        };
        assert_eq!(boot.timestamp, 1_000);
        assert_eq!(boot.capacity, capacity());
        assert!(!boot.boot_id.is_empty(), "store owns a fresh boot id");
        assert!(!dir.join("history.jsonl").exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_canonical_history_refuses_without_changing_its_bytes() {
        // This catches accepting a corrupt canonical file as if it were absent.
        let dir = std::env::temp_dir().join(format!(
            "nim-proxy-history-store-red-invalid-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("history-v1.jsonl");
        let before = b"{not canonical}\n";
        fs::write(&path, before).unwrap();

        let error = HistoryStore::open(&dir, 1_000, capacity()).unwrap_err();
        assert!(matches!(error, OpenError::InvalidCanonical { .. }));
        assert_eq!(fs::read(path).unwrap(), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn structurally_invalid_canonical_streams_refuse_before_fresh_boot_append() {
        // Task 11 is strict: recovery of bad epochs belongs to Task 12.
        let boot = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"boot\",\"timestamp\":10,\"boot_id\":\"boot-a\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]}}\n";
        let sample_a = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"sample\",\"timestamp\":11,\"boot_id\":\"boot-a\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]},\"state\":[]}\n";
        let sample_b = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"sample\",\"timestamp\":11,\"boot_id\":\"boot-b\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]},\"state\":[]}\n";
        let checkpoint_b = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"checkpoint\",\"timestamp\":11,\"boot_id\":\"boot-b\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]}}\n";
        let checkpoint_a = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"checkpoint\",\"timestamp\":11,\"boot_id\":\"boot-a\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]}}\n";
        let sample_mismatch = [boot.as_slice(), sample_b.as_slice()].concat();
        let checkpoint_mismatch = [boot.as_slice(), checkpoint_b.as_slice()].concat();
        let regression = [boot.as_slice(), sample_a.as_slice(), boot.as_slice()].concat();
        for (name, bytes) in [
            ("sample-before-boot", sample_a.as_slice()),
            ("sample-boot-mismatch", sample_mismatch.as_slice()),
            ("checkpoint-boot-mismatch", checkpoint_mismatch.as_slice()),
            ("timestamp-regression", regression.as_slice()),
        ] {
            let dir = test_dir(name);
            let path = dir.join("history-v1.jsonl");
            fs::write(&path, bytes).unwrap();
            assert!(HistoryStore::open(&dir, 20, capacity()).is_err(), "{name}");
            assert_eq!(fs::read(&path).unwrap(), bytes, "{name} stays evidence");
            let _ = fs::remove_dir_all(dir);
        }
        let dir = test_dir("checkpoint-after-boot");
        fs::write(
            dir.join("history-v1.jsonl"),
            [boot.as_slice(), checkpoint_a.as_slice()].concat(),
        )
        .unwrap();
        HistoryStore::open(&dir, 20, capacity()).unwrap();
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn final_canonical_record_requires_a_terminating_newline() {
        let dir = test_dir("unterminated-record");
        let path = dir.join("history-v1.jsonl");
        let bytes = b"{\"format\":\"nimproxy-history\",\"v\":1,\"kind\":\"boot\",\"timestamp\":1,\"boot_id\":\"boot-a\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]}}";
        fs::write(&path, bytes).unwrap();
        assert!(HistoryStore::open(&dir, 2, capacity()).is_err());
        assert_eq!(fs::read(path).unwrap(), bytes);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_parent_directory_is_created_for_first_publication() {
        // This catches assuming the caller pre-created DATA_DIR.
        let dir = test_dir("missing-parent").join("new").join("data");
        HistoryStore::open(&dir, 1, capacity()).unwrap();
        assert!(dir.join("history-v1.jsonl").is_file());
        let _ = fs::remove_dir_all(dir.parent().unwrap().parent().unwrap());
    }

    #[test]
    fn concurrent_first_open_has_one_published_boot_without_clobbering_it() {
        // This catches check-then-rename publication, which can replace a
        // concurrent creator's evidence.
        let dir = test_dir("concurrent");
        let first = std::thread::scope(|scope| {
            let left = scope.spawn(|| HistoryStore::open(&dir, 1, capacity()));
            let right = scope.spawn(|| HistoryStore::open(&dir, 1, capacity()));
            (left.join().unwrap(), right.join().unwrap())
        });
        assert!(
            first.0.is_ok() && first.1.is_ok(),
            "both creators can use the winner: left={:?} right={:?}",
            first.0,
            first.1
        );
        let records = fs::read_to_string(dir.join("history-v1.jsonl")).unwrap();
        assert_eq!(
            records.lines().count(),
            2,
            "each successful process owns one boot"
        );
        assert!(records
            .lines()
            .all(|line| decode_record(line.as_bytes()).is_ok()));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_temporary_is_ignored_and_left_as_evidence() {
        // This catches parsing or deleting a stale publication temporary.
        let dir = test_dir("stale-temp");
        let stale = dir.join("history-v1.jsonl.tmp-stale");
        let stale_bytes = b"{partial canonical boot evidence";
        fs::write(&stale, stale_bytes).unwrap();
        HistoryStore::open(&dir, 1, capacity()).unwrap();
        assert_eq!(fs::read(stale).unwrap(), stale_bytes);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn valid_restart_has_a_distinct_boot_id_for_its_first_checkpoint_and_sample() {
        // This catches using a caller-provided boot id or crossing a reset
        // boundary before the new process has written one.
        let dir = test_dir("restart");
        let mut first = HistoryStore::open(&dir, 1, capacity()).unwrap();
        first.append_sample(2, capacity(), state(1.0)).unwrap();
        drop(first);
        let mut second = HistoryStore::open(&dir, 3, capacity()).unwrap();
        second.checkpoint(4, capacity()).unwrap();
        second.append_sample(5, capacity(), state(1.0)).unwrap();
        let records = fs::read_to_string(dir.join("history-v1.jsonl")).unwrap();
        let records: Vec<_> = records
            .lines()
            .map(|line| decode_record(line.as_bytes()).unwrap())
            .collect();
        let Record::Boot(first_boot) = &records[0] else {
            panic!("first record must boot")
        };
        let Record::Boot(second_boot) = &records[2] else {
            panic!("restart must boot")
        };
        let Record::Checkpoint(first_after_restart) = &records[3] else {
            panic!("first restart record after boot must exercise checkpoint")
        };
        let Record::Sample(second_after_restart) = &records[4] else {
            panic!("restart state must be a full sample")
        };
        assert_ne!(
            first_boot.boot_id, second_boot.boot_id,
            "each process owns a new reset id"
        );
        assert_eq!(first_after_restart.boot_id, second_boot.boot_id);
        assert_eq!(second_after_restart.boot_id, second_boot.boot_id);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn invalid_empty_corrupt_and_future_files_remain_exactly_unchanged() {
        // This catches treating existing invalid evidence as a missing file.
        for (name, before) in [
            ("empty", b"".as_slice()),
            ("corrupt", b"{not json}\n".as_slice()),
            ("future", b"{\"format\":\"nimproxy-history\",\"v\":2,\"kind\":\"boot\",\"timestamp\":1,\"boot_id\":\"x\",\"capacity\":{\"capacity_rpm\":80,\"enabled_keys\":2,\"key_rpms\":[40,40]}}\n".as_slice()),
        ] {
            let dir = test_dir(name);
            let path = dir.join("history-v1.jsonl");
            fs::write(&path, before).unwrap();
            if name == "future" {
                assert_eq!(
                    decode_record(before.trim_ascii_end()).unwrap_err().diagnostic(),
                    "unsupported_version"
                );
            }
            assert!(HistoryStore::open(&dir, 2, capacity()).is_err());
            assert_eq!(fs::read(path).unwrap(), before, "{name} canonical evidence");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn canonical_non_file_path_refuses_without_replacing_it() {
        // This is deterministic under privileged test runners, unlike chmod:
        // a directory cannot be opened or replaced as the canonical file.
        let dir = test_dir("canonical-directory");
        let canonical = dir.join("history-v1.jsonl");
        fs::create_dir(&canonical).unwrap();
        assert!(HistoryStore::open(&dir, 1, capacity()).is_err());
        assert!(
            canonical.is_dir(),
            "failed open must not replace path evidence"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn legacy_file_is_never_parsed_or_mutated_when_canonical_is_absent_or_present() {
        // This catches accidental migration or a compatibility reader.
        for canonical in [false, true] {
            let dir = test_dir(if canonical {
                "legacy-plus-canonical"
            } else {
                "legacy-only"
            });
            let legacy = dir.join("history.jsonl");
            let legacy_bytes = b"legacy bytes stay opaque\n";
            fs::write(&legacy, legacy_bytes).unwrap();
            if canonical {
                HistoryStore::open(&dir, 1, capacity()).unwrap();
            }
            HistoryStore::open(&dir, 2, capacity()).unwrap();
            assert_eq!(fs::read(legacy).unwrap(), legacy_bytes);
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn changed_state_writes_sample_unchanged_state_writes_checkpoint_and_capacity_is_live() {
        // This catches re-emitting full idle snapshots or freezing capacity at open.
        let dir = test_dir("idle");
        let mut store = HistoryStore::open(&dir, 1, capacity()).unwrap();
        store.append_sample(2, capacity(), state(1.0)).unwrap();
        let changed_capacity = Capacity {
            capacity_rpm: 20,
            enabled_keys: 1,
            key_rpms: vec![20],
        };
        store
            .append_sample(3, changed_capacity.clone(), state(1.0))
            .unwrap();
        store
            .append_sample(4, changed_capacity.clone(), state(2.0))
            .unwrap();
        let records = fs::read_to_string(dir.join("history-v1.jsonl")).unwrap();
        let records: Vec<_> = records
            .lines()
            .map(|line| decode_record(line.as_bytes()).unwrap())
            .collect();
        assert!(matches!(records[2], Record::Checkpoint(_)));
        let Record::Checkpoint(checkpoint) = &records[2] else {
            unreachable!()
        };
        assert_eq!(checkpoint.capacity, changed_capacity);
        assert!(matches!(records[3], Record::Sample(_)));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn publication_failures_preserve_their_actual_crash_state_and_fire() {
        // This catches tests that merely configure a failure without proving
        // the intended operation was actually reached.
        for point in [
            FailurePoint::TempCreate,
            FailurePoint::TempWrite,
            FailurePoint::TempSync,
            FailurePoint::HardLink,
        ] {
            let dir = test_dir("injected");
            let canonical = dir.join("history-v1.jsonl");
            let failure = open_with_test_failure(&dir, 1, capacity(), point);
            assert!(failure.result.is_err(), "{point:?} must reject startup");
            assert_eq!(failure.fired, Some(point), "{point:?} injection must fire");
            assert!(!canonical.exists(), "{point:?} occurs before publication");
            if point != FailurePoint::TempCreate {
                assert!(
                    fs::read_dir(&dir).unwrap().any(|entry| entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .starts_with("history-v1.jsonl.tmp-")),
                    "{point:?} leaves its temporary evidence"
                );
            }
            let _ = fs::remove_dir_all(dir);
        }

        let dir = test_dir("directory-sync");
        let canonical = dir.join("history-v1.jsonl");
        let failure =
            open_with_test_failure(&dir, 1, capacity(), FailurePoint::ParentDirectorySync);
        assert!(failure.result.is_err());
        assert_eq!(failure.fired, Some(FailurePoint::ParentDirectorySync));
        let Record::Boot(_) = decode_record(&fs::read(&canonical).unwrap()).unwrap() else {
            panic!("directory-sync failure retains the linked complete boot evidence")
        };
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_and_sync_failures_do_not_hide_partial_tail_evidence() {
        // This catches truncate/repair after a failed restart boot append.
        for point in [
            FailurePoint::RestartBootAppend,
            FailurePoint::RestartBootSync,
        ] {
            let dir = test_dir("restart-failure");
            HistoryStore::open(&dir, 1, capacity()).unwrap();
            let before = fs::read(dir.join("history-v1.jsonl")).unwrap();
            let failure = open_with_test_failure(&dir, 2, capacity(), point);
            assert!(failure.result.is_err());
            assert_eq!(failure.fired, Some(point));
            let after = fs::read(dir.join("history-v1.jsonl")).unwrap();
            assert!(after.starts_with(&before), "failed tail remains evidence");
            if point == FailurePoint::RestartBootAppend {
                assert_eq!(after, before, "append failure writes no new tail");
            } else {
                assert!(
                    after.len() > before.len(),
                    "sync failure retains the partial new boot tail"
                );
            }
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn runtime_partial_append_poison_prevents_the_next_tick_from_mutating_history() {
        let dir = test_dir("runtime-poison");
        let mut store = HistoryStore::open(&dir, 1, capacity()).unwrap();
        let path = dir.join("history-v1.jsonl");
        let before = fs::read(&path).unwrap();
        let failed = append_with_test_failure(
            &mut store,
            2,
            capacity(),
            state(1.0),
            FailurePoint::RuntimePartialAppend,
        );
        assert!(failed.result.is_err());
        assert_eq!(failed.fired, Some(FailurePoint::RuntimePartialAppend));
        let after_failed_tick = fs::read(&path).unwrap();
        assert!(after_failed_tick.starts_with(&before));
        assert!(after_failed_tick.len() > before.len());
        assert!(!after_failed_tick.ends_with(b"\n"));
        assert!(store.append_sample(3, capacity(), state(2.0)).is_err());
        assert_eq!(fs::read(path).unwrap(), after_failed_tick);
        let _ = fs::remove_dir_all(dir);
    }
}
