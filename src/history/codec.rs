//! Canonical `nimproxy-history/v1` record serialization and validation.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_records_have_the_locked_field_order() {
        let record = decode_record(include_bytes!("../../tests/fixtures/history-v1/valid-sample.json"))
            .expect("the sanitized golden sample must decode");
        assert_eq!(
            encode_record(&record).expect("the decoded sample must encode"),
            include_bytes!("../../tests/fixtures/history-v1/valid-sample.json"),
        );
    }

    #[test]
    fn negative_fixtures_report_their_declared_diagnostic() {
        for (fixture, expected) in [
            ("truncated-json.json", "invalid_json"),
            ("invalid-utf8.bin", "invalid_utf8"),
            ("wrong-scalar-type.json", "invalid_record"),
            ("non-finite-number.json", "invalid_record"),
            ("duplicate-semantic-series.json", "duplicate_series"),
            ("unknown-state-kind.json", "invalid_state"),
            ("unknown-record-kind.json", "invalid_record_kind"),
            ("unknown-format.json", "unsupported_format"),
            ("unknown-version.json", "unsupported_version"),
        ] {
            let path = format!("tests/fixtures/history-v1/{fixture}");
            let bytes = std::fs::read(path).expect("fixture must exist");
            assert_eq!(decode_record(&bytes).unwrap_err().diagnostic(), expected);
        }
    }

    #[test]
    fn reordered_non_state_fields_decode_but_reencode_canonically() {
        let record = decode_record(include_bytes!(
            "../../tests/fixtures/history-v1/reordered-object-fields.json"
        ))
        .expect("unknown non-state fields are ignored");
        assert_eq!(
            encode_record(&record).expect("the valid record must encode"),
            include_bytes!("../../tests/fixtures/history-v1/valid-boot.json"),
        );
    }
}
