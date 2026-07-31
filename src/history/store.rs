//! Durable publication and append boundary for canonical history-v1.

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{open_with_test_failure, FailurePoint, HistoryStore, OpenError};
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
            let right = scope.spawn(|| HistoryStore::open(&dir, 2, capacity()));
            (left.join().unwrap(), right.join().unwrap())
        });
        assert!(
            first.0.is_ok() && first.1.is_ok(),
            "both creators can use the winner"
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
        let Record::Boot(first_boot) = &records[0] else { panic!("first record must boot") };
        let Record::Boot(second_boot) = &records[2] else { panic!("restart must boot") };
        let Record::Checkpoint(first_after_restart) = &records[3] else { panic!("first restart record after boot must exercise checkpoint") };
        let Record::Sample(second_after_restart) = &records[4] else { panic!("restart state must be a full sample") };
        assert_ne!(first_boot.boot_id, second_boot.boot_id, "each process owns a new reset id");
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
                assert_eq!(decode_record(before.trim_ascii_end()).unwrap_err().diagnostic(), "unsupported_version");
            }
            assert!(HistoryStore::open(&dir, 2, capacity()).is_err());
            assert_eq!(fs::read(path).unwrap(), before, "{name} canonical evidence");
            let _ = fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn canonical_non_file_path_refuses_without_replacing_it() {
        let dir = test_dir("canonical-directory");
        let canonical = dir.join("history-v1.jsonl");
        fs::create_dir(&canonical).unwrap();
        assert!(HistoryStore::open(&dir, 1, capacity()).is_err());
        assert!(canonical.is_dir(), "failed open must not replace path evidence");
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
        for point in [FailurePoint::Create, FailurePoint::Write, FailurePoint::Sync, FailurePoint::Link] {
            let dir = test_dir("injected");
            let canonical = dir.join("history-v1.jsonl");
            let failure = open_with_test_failure(&dir, 1, capacity(), point);
            assert!(failure.result.is_err(), "{point:?} must reject startup");
            assert_eq!(failure.fired, Some(point), "{point:?} injection must fire");
            assert!(!canonical.exists(), "{point:?} occurs before publication");
            if point != FailurePoint::Create {
                assert!(fs::read_dir(&dir).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with("history-v1.jsonl.tmp-")));
            }
            let _ = fs::remove_dir_all(dir);
        }
        let dir = test_dir("directory-sync");
        let canonical = dir.join("history-v1.jsonl");
        let failure = open_with_test_failure(&dir, 1, capacity(), FailurePoint::DirectorySync);
        assert!(failure.result.is_err());
        assert_eq!(failure.fired, Some(FailurePoint::DirectorySync));
        let Record::Boot(_) = decode_record(&fs::read(&canonical).unwrap()).unwrap() else { panic!("directory-sync failure retains linked boot") };
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn append_and_sync_failures_do_not_hide_partial_tail_evidence() {
        // This catches truncate/repair after a failed restart boot append.
        for point in [FailurePoint::Append, FailurePoint::Sync] {
            let dir = test_dir("restart-failure");
            HistoryStore::open(&dir, 1, capacity()).unwrap();
            let before = fs::read(dir.join("history-v1.jsonl")).unwrap();
            let failure = open_with_test_failure(&dir, 2, capacity(), point);
            assert!(failure.result.is_err());
            assert_eq!(failure.fired, Some(point));
            let after = fs::read(dir.join("history-v1.jsonl")).unwrap();
            assert!(after.starts_with(&before), "failed tail remains evidence");
            if point == FailurePoint::Append {
                assert_eq!(after, before, "append failure writes no new tail");
            } else {
                assert!(after.len() > before.len(), "sync failure retains partial new boot tail");
            }
            let _ = fs::remove_dir_all(dir);
        }
    }
}
