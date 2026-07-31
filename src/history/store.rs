//! Durable publication and append boundary for canonical history-v1.

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{HistoryStore, OpenError};
    use crate::history::codec::{decode_record, Capacity, Record};

    fn capacity() -> Capacity {
        Capacity {
            capacity_rpm: 80,
            enabled_keys: 2,
            key_rpms: vec![40, 40],
        }
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
}
