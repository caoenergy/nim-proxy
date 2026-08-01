//! Private, side-band NIM response observations.
//!
//! This RED scaffold deliberately returns unavailable outcomes.  The tests in
//! this module describe the evidence-backed behavior that replaces it.

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Observation {
    Measured(u64),
    Estimated(u64),
    Unavailable,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UsageObservations {
    pub(crate) cached_tokens: Observation,
    pub(crate) completion_tokens: Observation,
    pub(crate) prompt_tokens: Observation,
    pub(crate) reasoning_tokens: Observation,
    pub(crate) total_tokens: Observation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResponseObservations {
    pub(crate) finish_reasons: Vec<FinishObservation>,
    pub(crate) tool_calls: Observation,
    pub(crate) usage: UsageObservations,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FinishObservation {
    pub(crate) choice_index: u64,
    pub(crate) result: FinishResult,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FinishResult {
    Measured(FinishReason),
    Unavailable,
    Invalid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum FinishReason {
    ContentFilter,
    FunctionCall,
    Length,
    Other,
    Stop,
    ToolCalls,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StreamOutcome {
    Completed,
    Disconnected,
    Truncated,
}

#[derive(Default)]
pub(crate) struct SseObserver;

impl SseObserver {
    pub(crate) fn push(&mut self, _bytes: &[u8]) {}

    pub(crate) fn finish(self, _outcome: StreamOutcome) -> ResponseObservations {
        unavailable()
    }

    #[cfg(test)]
    fn retained_event_bytes(&self) -> usize {
        // Deliberately fails the RED bound assertion. GREEN must derive this
        // directly from the actual current-event storage.
        usize::MAX
    }
}

pub(crate) fn observe_buffered(_body: &[u8]) -> ResponseObservations {
    unavailable()
}

fn unavailable() -> ResponseObservations {
    ResponseObservations {
        finish_reasons: Vec::new(),
        tool_calls: Observation::Unavailable,
        usage: UsageObservations {
            cached_tokens: Observation::Unavailable,
            completion_tokens: Observation::Unavailable,
            prompt_tokens: Observation::Unavailable,
            reasoning_tokens: Observation::Unavailable,
            total_tokens: Observation::Unavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        observe_buffered, FinishObservation, FinishReason, FinishResult, Observation,
        ResponseObservations, SseObserver, StreamOutcome,
    };

    fn fixture_body(name: &str) -> Vec<u8> {
        let fixture = match name {
            "buffered-basic" => {
                include_str!("../tests/fixtures/nim-observations/buffered-basic.json")
            }
            "buffered-tools" => {
                include_str!("../tests/fixtures/nim-observations/buffered-tools.json")
            }
            "streamed-basic" => {
                include_str!("../tests/fixtures/nim-observations/streamed-basic.json")
            }
            "streamed-tools" => {
                include_str!("../tests/fixtures/nim-observations/streamed-tools.json")
            }
            _ => panic!("unknown literal evidence fixture: {name}"),
        };
        serde_json::from_str::<serde_json::Value>(fixture).unwrap()["body"]
            .as_str()
            .unwrap()
            .as_bytes()
            .to_vec()
    }

    fn assert_fixture_outcome(
        actual: ResponseObservations,
        finish: FinishReason,
        tools: u64,
        production_mutation: &str,
    ) {
        assert_eq!(
            actual.usage.prompt_tokens,
            Observation::Measured(1),
            "{production_mutation}: prompt must come from the literal fixture"
        );
        assert_eq!(
            actual.usage.completion_tokens,
            Observation::Measured(2),
            "{production_mutation}: completion must come from the literal fixture"
        );
        assert_eq!(
            actual.usage.total_tokens,
            Observation::Measured(3),
            "{production_mutation}: total must come from the literal fixture"
        );
        assert_eq!(actual.usage.cached_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Unavailable);
        assert_eq!(actual.tool_calls, Observation::Measured(tools));
        assert_eq!(
            actual.finish_reasons,
            vec![FinishObservation {
                choice_index: 0,
                result: FinishResult::Measured(finish),
            }],
            "{production_mutation}: finish observation must stay choice-indexed"
        );
    }

    #[test]
    fn buffered_fixture_conformance_rejects_old_first_choice_and_usage_shortcuts() {
        // Mutation caught: retaining the old ad-hoc buffered extraction loses
        // typed unavailable fields and does not make choice/tool classification
        // a checked observation boundary.
        assert_fixture_outcome(
            observe_buffered(&fixture_body("buffered-basic")),
            FinishReason::Stop,
            0,
            "old buffered shortcut",
        );
        assert_fixture_outcome(
            observe_buffered(&fixture_body("buffered-tools")),
            FinishReason::ToolCalls,
            1,
            "old buffered shortcut",
        );
    }

    #[test]
    fn streamed_fixture_conformance_rejects_old_line_scanner() {
        // Mutation caught: the previous line scanner treats a null progress
        // reason and the usage-only final event as unrelated observations.
        for (name, finish, tools) in [
            ("streamed-basic", FinishReason::Stop, 0),
            ("streamed-tools", FinishReason::ToolCalls, 1),
        ] {
            let mut observer = SseObserver::default();
            observer.push(&fixture_body(name));
            assert_fixture_outcome(
                observer.finish(StreamOutcome::Completed),
                finish,
                tools,
                "old SseScan line scanner",
            );
        }
    }

    #[test]
    fn usage_integer_boundaries_reject_non_u64_without_silent_casting() {
        // Mutation caught: serde Value::as_u64-style extraction silently drops
        // malformed presentations rather than emitting Invalid for that field.
        for (body, expected) in [
            (
                br#"{"usage":{"prompt_tokens":0,"completion_tokens":18446744073709551615}}"#
                    .as_slice(),
                (Observation::Measured(0), Observation::Measured(u64::MAX)),
            ),
            (
                br#"{"usage":{"prompt_tokens":18446744073709551616,"completion_tokens":-1}}"#
                    .as_slice(),
                (Observation::Invalid, Observation::Invalid),
            ),
            (
                br#"{"usage":{"prompt_tokens":1.0,"completion_tokens":"2"}}"#.as_slice(),
                (Observation::Invalid, Observation::Invalid),
            ),
            (
                br#"{"usage":{"prompt_tokens":null,"completion_tokens":{}}}"#.as_slice(),
                (Observation::Invalid, Observation::Invalid),
            ),
        ] {
            let actual = observe_buffered(body);
            assert_eq!(
                actual.usage.prompt_tokens, expected.0,
                "usage integer boundary"
            );
            assert_eq!(
                actual.usage.completion_tokens, expected.1,
                "usage integer boundary"
            );
        }

        let actual = observe_buffered(br#"{"usage":[]}"#);
        assert_eq!(actual.usage.prompt_tokens, Observation::Invalid);
        assert_eq!(actual.usage.completion_tokens, Observation::Invalid);
        assert_eq!(actual.usage.total_tokens, Observation::Invalid);
        assert_eq!(actual.usage.cached_tokens, Observation::Invalid);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Invalid);
    }

    #[test]
    fn stream_observer_reassembles_crlf_multiline_comments_and_every_split() {
        // Mutation caught: line-local parsing misses a valid event split at an
        // arbitrary transport boundary or changes the standard SSE data join.
        let bytes = b": keepalive\r\n\r\ndata: {\"choices\":[{\"index\":0,\r\ndata: \"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":3}}\r\n\r\n";
        for split in 0..=bytes.len() {
            let mut observer = SseObserver::default();
            observer.push(&bytes[..split]);
            observer.push(&bytes[split..]);
            assert_fixture_outcome(
                observer.finish(StreamOutcome::Completed),
                FinishReason::Stop,
                0,
                "split/CRLF/multi-line SSE parser",
            );
        }
    }

    #[test]
    fn stream_observer_invalidates_duplicate_conflicts_and_relationships() {
        // Mutation caught: first-value-wins scanning accepts contradictory
        // duplicate usage, cached, or reasoning presentations.
        let mut observer = SseObserver::default();
        observer.push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":3},\"completion_tokens_details\":{\"reasoning_tokens\":3}}}\n\n");
        let actual = observer.finish(StreamOutcome::Completed);
        assert_eq!(actual.usage.prompt_tokens, Observation::Invalid);
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(2));
        assert_eq!(actual.usage.cached_tokens, Observation::Invalid);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Invalid);

        let mut equal_duplicates = SseObserver::default();
        equal_duplicates.push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n");
        let actual = equal_duplicates.finish(StreamOutcome::Completed);
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(2));
    }

    #[test]
    fn relationship_matrix_invalidates_only_dependent_usage_fields() {
        // Mutation caught: accepting unchecked totals or discarding healthy
        // siblings after one sum/parent relationship is invalid.
        let actual = observe_buffered(b"{\"usage\":{\"prompt_tokens\":18446744073709551615,\"completion_tokens\":1,\"total_tokens\":18446744073709551615}}");
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(u64::MAX));
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.total_tokens, Observation::Invalid);
        assert_eq!(actual.usage.cached_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Unavailable);

        let actual = observe_buffered(
            b"{\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"total_tokens\":2}}",
        );
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(2));
        assert_eq!(actual.usage.total_tokens, Observation::Invalid);

        let actual = observe_buffered(
            b"{\"usage\":{\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":1}}}",
        );
        assert_eq!(actual.usage.prompt_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(2));
        assert_eq!(actual.usage.cached_tokens, Observation::Invalid);

        let actual = observe_buffered(
            b"{\"usage\":{\"prompt_tokens\":\"1\",\"completion_tokens\":2,\"prompt_tokens_details\":{\"cached_tokens\":1}}}",
        );
        assert_eq!(actual.usage.prompt_tokens, Observation::Invalid);
        assert_eq!(actual.usage.completion_tokens, Observation::Measured(2));
        assert_eq!(actual.usage.cached_tokens, Observation::Invalid);

        let actual = observe_buffered(
            b"{\"usage\":{\"prompt_tokens\":1,\"completion_tokens_details\":{\"reasoning_tokens\":1}}}",
        );
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.completion_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Invalid);

        let actual = observe_buffered(
            b"{\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":\"2\",\"completion_tokens_details\":{\"reasoning_tokens\":1}}}",
        );
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.completion_tokens, Observation::Invalid);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Invalid);

        let mut observer = SseObserver::default();
        observer.push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2,\"completion_tokens_details\":{\"reasoning_tokens\":1}}}\n\ndata: {\"choices\":[],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":3}}\n\n");
        let actual = observer.finish(StreamOutcome::Completed);
        assert_eq!(actual.usage.prompt_tokens, Observation::Measured(1));
        assert_eq!(actual.usage.completion_tokens, Observation::Invalid);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Invalid);
    }

    #[test]
    fn choice_and_tool_observations_reject_old_first_choice_and_max_index_rules() {
        // Mutation caught: first-choice-only finish extraction and max-index
        // tool counting lose valid choices, accept malformed shapes, and count
        // duplicate stream fragments as separate calls.
        let actual = observe_buffered(b"{\"choices\":[{\"index\":1,\"message\":{},\"finish_reason\":\"length\"},{\"index\":0,\"message\":{},\"finish_reason\":\"content_filter\"}]} ");
        assert_eq!(
            actual.finish_reasons,
            vec![
                FinishObservation {
                    choice_index: 0,
                    result: FinishResult::Measured(FinishReason::ContentFilter),
                },
                FinishObservation {
                    choice_index: 1,
                    result: FinishResult::Measured(FinishReason::Length),
                },
            ]
        );
        assert_eq!(actual.tool_calls, Observation::Measured(0));

        let actual = observe_buffered(b"{\"choices\":[]}");
        assert_eq!(actual.tool_calls, Observation::Measured(0));
        let actual = observe_buffered(b"{}");
        assert_eq!(actual.tool_calls, Observation::Unavailable);
        let actual = observe_buffered(b"{\"choices\":{}}");
        assert_eq!(actual.tool_calls, Observation::Invalid);

        let actual = observe_buffered(b"{\"choices\":[{\"index\":0,\"message\":{\"tool_calls\":[{},{}]}},{\"index\":1,\"message\":{\"tool_calls\":[{},{},{}]}}]}");
        assert_eq!(actual.tool_calls, Observation::Measured(5));

        let actual = observe_buffered(b"{\"choices\":[7]}");
        assert_eq!(actual.tool_calls, Observation::Invalid);
        assert!(actual.finish_reasons.is_empty());

        let actual = observe_buffered(
            b"{\"choices\":[{\"index\":0,\"message\":7,\"finish_reason\":\"stop\"}]}",
        );
        assert_eq!(actual.tool_calls, Observation::Invalid);
        assert_eq!(
            actual.finish_reasons,
            vec![FinishObservation {
                choice_index: 0,
                result: FinishResult::Measured(FinishReason::Stop),
            }]
        );

        let actual = observe_buffered(
            b"{\"choices\":[{\"index\":0,\"message\":{\"tool_calls\":{}},\"finish_reason\":\"stop\"}]}",
        );
        assert_eq!(actual.tool_calls, Observation::Invalid);
        assert_eq!(
            actual.finish_reasons,
            vec![FinishObservation {
                choice_index: 0,
                result: FinishResult::Measured(FinishReason::Stop),
            }]
        );

        for body in [
            b"{\"choices\":[{\"message\":{}}]}".as_slice(),
            b"{\"choices\":[{\"index\":-1,\"message\":{}}]}".as_slice(),
            b"{\"choices\":[{\"index\":1.5,\"message\":{}}]}".as_slice(),
            b"{\"choices\":[{\"index\":18446744073709551616,\"message\":{}}]}".as_slice(),
        ] {
            let actual = observe_buffered(body);
            assert_eq!(actual.tool_calls, Observation::Invalid);
            assert!(
                actual.finish_reasons.is_empty(),
                "unrepresentable choice index must not fabricate a finish observation"
            );
        }

        let mut observer = SseObserver::default();
        observer.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0},{\"index\":0}]}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1}]}}]}\n\n");
        assert_eq!(
            observer.finish(StreamOutcome::Completed).tool_calls,
            Observation::Measured(2)
        );

        let mut malformed = SseObserver::default();
        malformed.push(
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":-1}]}}]}\n\n",
        );
        assert_eq!(
            malformed.finish(StreamOutcome::Completed).tool_calls,
            Observation::Invalid
        );

        for bytes in [
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[7]}}]}\n\n"
                .as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{}]}}]}\n\n"
                .as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":-1}]}}]}\n\n"
                .as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1.5}]}}]}\n\n"
                .as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":18446744073709551616}]}}]}\n\n"
                .as_slice(),
        ] {
            let mut observer = SseObserver::default();
            observer.push(bytes);
            assert_eq!(
                observer.finish(StreamOutcome::Completed).tool_calls,
                Observation::Invalid,
                "malformed streamed tool-call presentation must be invalid"
            );
        }

        for bytes in [
            b"data: {\"choices\":{}}\n\n".as_slice(),
            b"data: {\"choices\":[7]}\n\n".as_slice(),
        ] {
            let mut observer = SseObserver::default();
            observer.push(bytes);
            let actual = observer.finish(StreamOutcome::Completed);
            assert_eq!(actual.tool_calls, Observation::Invalid);
            assert!(
                actual.finish_reasons.is_empty(),
                "malformed streamed choices must not fabricate finish observations"
            );
        }
    }

    #[test]
    fn finish_taxonomy_and_completed_estimate_reject_silent_terminal_fallbacks() {
        // Mutation caught: treating unknown or malformed terminal values as a
        // first-choice string and estimating regardless of terminal validity.
        let actual = observe_buffered(b"{\"choices\":[{\"index\":0,\"message\":{},\"finish_reason\":\"function_call\"},{\"index\":1,\"message\":{},\"finish_reason\":\"future_reason\"},{\"index\":2,\"message\":{},\"finish_reason\":1},{\"index\":3,\"message\":{}}]}");
        assert_eq!(
            actual.finish_reasons,
            vec![
                FinishObservation {
                    choice_index: 0,
                    result: FinishResult::Measured(FinishReason::FunctionCall),
                },
                FinishObservation {
                    choice_index: 1,
                    result: FinishResult::Measured(FinishReason::Other),
                },
                FinishObservation {
                    choice_index: 2,
                    result: FinishResult::Invalid,
                },
                FinishObservation {
                    choice_index: 3,
                    result: FinishResult::Unavailable,
                },
            ]
        );

        let mut observer = SseObserver::default();
        observer.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\n");
        assert_eq!(
            observer
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Estimated(1)
        );
    }

    #[test]
    fn completed_stream_estimate_counts_only_valid_nonterminal_choice_events() {
        // Mutation caught: the old scanner counts every data line, estimates
        // zero, or replaces invalid/measured completion with an estimate.
        let mut zero = SseObserver::default();
        zero.push(b"data: [DONE]\n\ndata: {\"choices\":[]}\n\n");
        assert_eq!(
            zero.finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Unavailable
        );

        let mut measured = SseObserver::default();
        measured.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":7}}\n\n");
        assert_eq!(
            measured
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Measured(7)
        );

        let mut invalid = SseObserver::default();
        invalid.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{}}],\"usage\":{\"completion_tokens\":\"2\"}}\n\n");
        assert_eq!(
            invalid
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Invalid
        );

        let mut conflicting = SseObserver::default();
        conflicting.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{}}],\"usage\":{\"completion_tokens\":2}}\n\ndata: {\"choices\":[],\"usage\":{\"completion_tokens\":3}}\n\n");
        assert_eq!(
            conflicting
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Invalid
        );

        let mut usage_only = SseObserver::default();
        usage_only.push(b"data: {\"choices\":[],\"usage\":{\"completion_tokens\":2}}\n\n");
        assert_eq!(
            usage_only
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Measured(2),
            "usage-only event supplies measured completion but does not estimate"
        );

        let mut comment_only = SseObserver::default();
        comment_only.push(b": keepalive\n\n");
        assert_eq!(
            comment_only
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Unavailable,
            "comment-only event must not count toward completion estimate"
        );

        let mut json_error = SseObserver::default();
        json_error.push(b"data: {\"error\":{\"message\":\"redacted\"}}\n\n");
        assert_eq!(
            json_error
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Unavailable,
            "parsed JSON error event must not count toward completion estimate"
        );

        let mut usage_without_completion = SseObserver::default();
        usage_without_completion
            .push(b"data: {\"choices\":[],\"usage\":{\"prompt_tokens\":1}}\n\n");
        assert_eq!(
            usage_without_completion
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Unavailable,
            "usage-only event without completion tokens must not estimate completion"
        );

        for bytes in [
            b"data: {]\n\n".as_slice(),
            b"data: \xff\n\n".as_slice(),
            b"data: [DONE]\n\n".as_slice(),
            b"data: {\"choices\":[]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n".as_slice(),
            b"data: {\"choices\":[{\"index\":0,\"finish_reason\":1}]}\n\n".as_slice(),
        ] {
            let mut observer = SseObserver::default();
            observer.push(bytes);
            assert_eq!(
                observer
                    .finish(StreamOutcome::Completed)
                    .usage
                    .completion_tokens,
                Observation::Unavailable,
                "non-countable literal SSE event must not estimate completion"
            );
        }

        let mut counted = SseObserver::default();
        counted.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\ndata: {\"choices\":[{\"index\":1,\"delta\":{}}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\n");
        assert_eq!(
            counted
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Estimated(3)
        );
    }

    #[test]
    fn completed_unterminated_final_event_has_truncated_observation_outcome() {
        // Mutation caught: clean EOF publishes a partial final SSE event as
        // measured or estimated instead of treating the observation as truncated.
        let mut observer = SseObserver::default();
        observer.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}");
        let actual = observer.finish(StreamOutcome::Completed);
        assert_eq!(actual.usage.prompt_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.completion_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.total_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.cached_tokens, Observation::Unavailable);
        assert_eq!(actual.usage.reasoning_tokens, Observation::Unavailable);
        assert_eq!(actual.tool_calls, Observation::Unavailable);
        assert_eq!(
            actual.finish_reasons,
            vec![FinishObservation {
                choice_index: 0,
                result: FinishResult::Unavailable,
            }]
        );
    }

    #[test]
    fn stream_observer_discards_numeric_partial_observations_on_disconnect_or_truncation() {
        // Mutation caught: end-of-loop accounting publishes partial measured or
        // estimated usage after a client disconnect or unterminated final event.
        for outcome in [StreamOutcome::Disconnected, StreamOutcome::Truncated] {
            let mut observer = SseObserver::default();
            observer.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":1,\"completion_tokens\":2}}\n\n");
            let actual = observer.finish(outcome);
            assert_eq!(actual.usage.prompt_tokens, Observation::Unavailable);
            assert_eq!(actual.usage.completion_tokens, Observation::Unavailable);
            assert_eq!(actual.tool_calls, Observation::Unavailable);
            assert_eq!(
                actual.finish_reasons,
                vec![FinishObservation {
                    choice_index: 0,
                    result: FinishResult::Unavailable,
                }]
            );
        }
    }

    #[test]
    fn unobservable_events_are_bounded_noncountable_and_do_not_block_recovery() {
        // Mutation caught: retaining an over-bound event or counting malformed
        // bytes makes the observer exceed its memory bound or inflate estimates.
        let mut observer = SseObserver::default();
        let mut over_bound = b"data: ".to_vec();
        over_bound.extend(std::iter::repeat_n(b'x', 1_048_577));
        observer.push(&over_bound);
        assert!(
            observer.retained_event_bytes() <= 1_048_576,
            "over-bound current SSE event must be discarded rather than retained"
        );
        observer.push(b"\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\n");
        assert_eq!(
            observer
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Estimated(1),
            "discarded over-bound event must not count and later framed input must recover"
        );

        let mut malformed = SseObserver::default();
        malformed.push(b"data: {]\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\n");
        assert_eq!(
            malformed
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Estimated(1),
            "malformed JSON event must be unobservable and non-countable"
        );

        let mut invalid_utf8 = SseObserver::default();
        invalid_utf8.push(b"data: \xff\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{}}]}\n\n");
        assert_eq!(
            invalid_utf8
                .finish(StreamOutcome::Completed)
                .usage
                .completion_tokens,
            Observation::Estimated(1),
            "invalid UTF-8 event must be unobservable and non-countable"
        );
    }
}
