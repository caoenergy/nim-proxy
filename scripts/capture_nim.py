#!/usr/bin/env python3
"""Capture one bounded private NIM response (implementation follows this RED contract)."""

import argparse
import copy
import os
import pathlib
import stat
import sys
import tempfile
import urllib.parse
from dataclasses import dataclass


CASES = (
    "buffered-basic",
    "streamed-basic",
    "buffered-tools",
    "streamed-tools",
)
REQUIRED_ENVIRONMENT = (
    "NIM_CAPTURE_BASE_URL",
    "NIM_CAPTURE_BEARER_TOKEN",
    "NIM_CAPTURE_MODEL",
)
MAX_RESPONSE_BYTES = 2 * 1024 * 1024
MAX_SSE_EVENTS = 2_048
MAX_CAPTURE_SECONDS = 60
OWNER_DIRECTORY_MODE = 0o700
OWNER_FILE_MODE = 0o600
EXCLUSIVE_NOFOLLOW_FLAGS = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
FIXED_TOOL = {
    "type": "function",
    "function": {
        "name": "record_capture_marker",
        "description": "Record the fixed capture marker.",
        "parameters": {
            "type": "object",
            "properties": {},
            "additionalProperties": False,
        },
    },
}


@dataclass(frozen=True, order=True)
class Problem:
    check: str


@dataclass(frozen=True)
class CandidateDecision:
    """Current deliberately incomplete production-boundary output."""

    rejected: bool = False
    truncated: bool = False
    file_flags: int = 0


@dataclass(frozen=True)
class CaptureRequestPlan:
    """The only request body and its separate non-JSON execution policy."""

    body: dict
    attempts: int = 0
    retries: int = 0


def candidate_environment(environment: dict[str, str]) -> CandidateDecision:
    """GREEN will inspect only the three approved environment names."""
    del environment
    return CandidateDecision()


def candidate_url(base_url: str) -> CandidateDecision:
    """GREEN will reject unsafe service roots before any transport opens."""
    del base_url
    return CandidateDecision()


def candidate_path(
    output_dir: pathlib.Path, repository: pathlib.Path
) -> CandidateDecision:
    """GREEN will create only a new, resolved-outside-repository directory."""
    del output_dir, repository
    return CandidateDecision(file_flags=0)


def candidate_owner_mode(path: pathlib.Path, expected_mode: int) -> CandidateDecision:
    """GREEN will reject pre-existing owner-mode drift before writing."""
    del path, expected_mode
    return CandidateDecision()


def candidate_limit(
    response_bytes: int, event_count: int, elapsed_seconds: int
) -> CandidateDecision:
    """GREEN will stop and mark bounded truncation at any locked limit."""
    del response_bytes, event_count, elapsed_seconds
    return CandidateDecision()


def candidate_diagnostic(channel: str, secret: str) -> str:
    """Current RED output is intentionally unsafe; it is never printed."""
    del channel
    return secret


def candidate_request_profile(case: str, model: str) -> CaptureRequestPlan:
    """GREEN will build exactly one fixed request profile for each case."""
    del case, model
    return CaptureRequestPlan(body={})


def candidate_capture() -> None:
    """The CLI shape is frozen; live capture remains absent during RED."""
    return None


def exact_boundary_sets(
    required: bool, observed: bool, check: str
) -> tuple[set[Problem], set[Problem]]:
    """Translate an opaque candidate verdict into a stable oracle check id."""
    expected = {Problem(check)} if required else set()
    actual = {Problem(check)} if observed else set()
    return expected, actual


def oracle_environment(environment: dict[str, str]) -> bool:
    return not all(name in environment for name in REQUIRED_ENVIRONMENT)


def oracle_url(base_url: str) -> bool:
    parsed = urllib.parse.urlsplit(base_url)
    loopback = {"127.0.0.1", "::1"}
    safe = (
        parsed.scheme == "https"
        or (parsed.scheme == "http" and parsed.hostname in loopback)
    ) and not (
        parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
    )
    return not safe


def oracle_path(
    output_dir: pathlib.Path,
    repository: pathlib.Path,
    require_exclusive_nofollow: bool,
) -> bool:
    resolved_repository = repository.resolve()
    resolved_output = output_dir.resolve(strict=False)
    lexical_repository = repository.absolute()
    lexical_output = output_dir.absolute()
    has_symlink_component = output_dir.is_symlink() or any(
        parent.is_symlink() for parent in output_dir.parents
    )
    outside_repository_lexically = not lexical_output.is_relative_to(
        lexical_repository
    )
    outside_repository = not resolved_output.is_relative_to(resolved_repository)
    safe = (
        output_dir.is_absolute()
        and not output_dir.exists()
        and not has_symlink_component
        and outside_repository_lexically
        and outside_repository
    )
    del require_exclusive_nofollow
    return not safe


def oracle_file_flags(decision: CandidateDecision) -> set[Problem]:
    if decision.file_flags == EXCLUSIVE_NOFOLLOW_FLAGS:
        return set()
    return {Problem("capture-path-boundary")}


def oracle_owner_mode(
    path: pathlib.Path,
    expected_mode: int,
) -> bool:
    actual_mode = stat.S_IMODE(path.stat().st_mode)
    return actual_mode != expected_mode


def oracle_limit(
    response_bytes: int,
    event_count: int,
    elapsed_seconds: int,
) -> bool:
    return (
        response_bytes > MAX_RESPONSE_BYTES
        or event_count > MAX_SSE_EVENTS
        or elapsed_seconds > MAX_CAPTURE_SECONDS
    )


def oracle_diagnostic(channel: str, diagnostic: str, secret: str) -> set[Problem]:
    if secret not in diagnostic:
        return set()
    return {Problem(f"capture-secret-leak:{channel}")}


def oracle_request_profile(
    case: str,
    model: str,
    plan: CaptureRequestPlan,
    prompt: object | None,
) -> set[Problem]:
    streamed = case.startswith("streamed-")
    tools = case.endswith("-tools")
    body = plan.body
    expected_keys = {"max_tokens", "messages", "model", "stream"}
    if streamed:
        expected_keys.add("stream_options")
    if tools:
        expected_keys.update({"tool_choice", "tools"})
    problems = set()
    if set(body) != expected_keys:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if body.get("model") != model:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if body.get("stream") is not streamed:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if plan.attempts != 1 or plan.retries != 0:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if not isinstance(body.get("max_tokens"), int) or body["max_tokens"] <= 0:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if not isinstance(body.get("messages"), list) or not body["messages"]:
        problems.add(Problem(f"capture-request-profile:{case}"))
    elif prompt is not None and body["messages"] != prompt:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if streamed:
        if body.get("stream_options") != {"include_usage": True}:
            problems.add(Problem(f"capture-request-profile:{case}"))
    elif "stream_options" in body:
        problems.add(Problem(f"capture-request-profile:{case}"))
    if tools:
        tool_list = body.get("tools")
        tool_choice = body.get("tool_choice")
        if tool_list != [FIXED_TOOL] or tool_choice != "required":
            problems.add(Problem(f"capture-request-profile:{case}"))
    elif "tools" in body or "tool_choice" in body:
        problems.add(Problem(f"capture-request-profile:{case}"))
    return problems


def oracle_control_plan(case: str, model: str) -> CaptureRequestPlan:
    """A synthetic complete plan used only to prove this oracle can fail."""
    body = {
        "max_tokens": 1,
        "messages": [{"content": "fixture", "role": "user"}],
        "model": model,
        "stream": case.startswith("streamed-"),
    }
    if case.startswith("streamed-"):
        body["stream_options"] = {"include_usage": True}
    if case.endswith("-tools"):
        body["tools"] = [copy.deepcopy(FIXED_TOOL)]
        body["tool_choice"] = "required"
    return CaptureRequestPlan(body=body, attempts=1, retries=0)


def checks(problems: set[Problem]) -> list[str]:
    return [problem.check for problem in sorted(problems)]


def expect_exact(
    name: str, expected: set[Problem], observed: set[Problem], failures: list[str]
) -> None:
    """GREEN expectation: candidate output has the exact fixed oracle checks."""
    if observed != expected:
        failures.append(
            f"{name}: expected {checks(expected)}; observed {checks(observed)}"
        )
    else:
        print(f"  ok  {name:34} candidate matches its fixed oracle")


def selftest() -> int:
    """Exercise frozen hostile inputs without touching a real environment or network."""
    failures: list[str] = []
    sentinel = "capture-secret-sentinel-not-a-credential"
    with tempfile.TemporaryDirectory(prefix="nim-capture-selftest-") as temporary:
        root = pathlib.Path(temporary)
        repository = root / "repository"
        repository.mkdir()
        existing_directory = root / "existing-directory"
        existing_directory.mkdir()
        existing_file = root / "existing-file"
        existing_file.write_bytes(b"fixture")
        alias = root / "repository-alias"
        alias.symlink_to(repository, target_is_directory=True)
        outside_parent = root / "outside-parent"
        outside_parent.mkdir()
        outside_alias = root / "outside-alias"
        outside_alias.symlink_to(outside_parent, target_is_directory=True)
        valid_directory = root / "new-owner-only-output"
        valid_mode_directory = root / "owner-only-directory"
        valid_mode_directory.mkdir(mode=OWNER_DIRECTORY_MODE)
        os.chmod(valid_mode_directory, OWNER_DIRECTORY_MODE)
        valid_mode_file = root / "owner-only-file"
        valid_mode_file.write_bytes(b"fixture")
        os.chmod(valid_mode_file, OWNER_FILE_MODE)
        drift_directory = root / "permission-drift-directory"
        drift_directory.mkdir(mode=0o755)
        drift_file = root / "permission-drift-file"
        drift_file.write_bytes(b"fixture")
        os.chmod(drift_file, 0o644)
        nofollow_target = root / "nofollow-target"
        nofollow_target.write_bytes(b"fixture")
        nofollow_alias = root / "nofollow-alias"
        nofollow_alias.symlink_to(nofollow_target)

        complete_environment = {
            name: "temporary-sentinel" for name in REQUIRED_ENVIRONMENT
        }
        boundary_cases = (
            (
                "complete-temporary-environment",
                *exact_boundary_sets(
                    oracle_environment(complete_environment),
                    candidate_environment(complete_environment).rejected,
                    "capture-environment",
                ),
            ),
            (
                "missing-environment",
                *exact_boundary_sets(
                    oracle_environment({}), candidate_environment({}).rejected,
                    "capture-environment",
                ),
            ),
            (
                "direct-https-service-root",
                *exact_boundary_sets(
                    oracle_url("https://capture.example.invalid/v1"),
                    candidate_url("https://capture.example.invalid/v1").rejected,
                    "capture-url-boundary",
                ),
            ),
            (
                "literal-loopback-http-service-root",
                *exact_boundary_sets(
                    oracle_url("http://127.0.0.1:8000/v1"),
                    candidate_url("http://127.0.0.1:8000/v1").rejected,
                    "capture-url-boundary",
                ),
            ),
            (
                "forbidden-http-non-loopback",
                *exact_boundary_sets(
                    oracle_url("http://capture.example.invalid/v1"),
                    candidate_url("http://capture.example.invalid/v1").rejected,
                    "capture-url-boundary",
                ),
            ),
            (
                "forbidden-url-components",
                *exact_boundary_sets(
                    oracle_url("https://user:password@example.invalid/v1?query#fragment"),
                    candidate_url(
                        "https://user:password@example.invalid/v1?query#fragment"
                    ).rejected,
                    "capture-url-boundary",
                ),
            ),
        )

        path_cases = (
            ("relative-output-directory", pathlib.Path("relative-output"), False),
            ("new-absolute-outside-repository-output", valid_directory, False),
            ("existing-output-directory", existing_directory, False),
            ("existing-output-file", existing_file, False),
            ("inside-repository-output", repository / "raw", False),
            (
                "lexically-inside-repository-output",
                repository / ".." / "lexically-escaped-output",
                False,
            ),
            ("symlink-aliased-output", alias / "raw", False),
            ("outside-symlink-aliased-output", outside_alias / "raw", False),
            ("exclusive-nofollow-output-file", nofollow_alias, True),
        )
        mode_cases = (
            ("directory-permission-drift", drift_directory, OWNER_DIRECTORY_MODE),
            ("owner-only-directory", valid_mode_directory, OWNER_DIRECTORY_MODE),
            ("file-permission-drift", drift_file, OWNER_FILE_MODE),
            ("owner-only-file", valid_mode_file, OWNER_FILE_MODE),
        )
        limit_cases = (
            ("limits-below-bound", MAX_RESPONSE_BYTES, MAX_SSE_EVENTS, MAX_CAPTURE_SECONDS),
            ("response-byte-limit", MAX_RESPONSE_BYTES + 1, 0, 0),
            ("sse-event-limit", 0, MAX_SSE_EVENTS + 1, 0),
            ("wall-clock-limit", 0, 0, MAX_CAPTURE_SECONDS + 1),
        )
        for name, expected, observed in boundary_cases:
            expect_exact(name, expected, observed, failures)
        for name, output_dir, require_exclusive_nofollow in path_cases:
            decision = candidate_path(output_dir, repository)
            expected, observed = exact_boundary_sets(
                oracle_path(output_dir, repository, require_exclusive_nofollow),
                decision.rejected,
                "capture-path-boundary",
            )
            expect_exact(name, expected, observed, failures)
        expect_exact(
            "exclusive-nofollow-file-policy",
            set(),
            oracle_file_flags(candidate_path(valid_directory, repository)),
            failures,
        )
        for name, path, expected_mode in mode_cases:
            decision = candidate_owner_mode(path, expected_mode)
            expected, observed = exact_boundary_sets(
                oracle_owner_mode(path, expected_mode),
                decision.rejected,
                "capture-owner-mode",
            )
            expect_exact(name, expected, observed, failures)
        for name, response_bytes, event_count, elapsed_seconds in limit_cases:
            decision = candidate_limit(response_bytes, event_count, elapsed_seconds)
            expected, observed = exact_boundary_sets(
                oracle_limit(response_bytes, event_count, elapsed_seconds),
                decision.truncated,
                "capture-limit",
            )
            expect_exact(name, expected, observed, failures)

        for channel in ("stdout", "stderr", "exception"):
            expect_exact(
                f"secret-free-{channel}",
                set(),
                oracle_diagnostic(channel, candidate_diagnostic(channel, sentinel), sentinel),
                failures,
            )

        configured_model = "capture-model-sentinel"
        profiles = {
            case: candidate_request_profile(case, configured_model) for case in CASES
        }
        prompt = profiles["buffered-basic"].body.get("messages")
        for case, plan in profiles.items():
            expect_exact(
                f"request-profile-{case}",
                set(),
                oracle_request_profile(case, configured_model, plan, prompt),
                failures,
            )

        control_case = "buffered-basic"
        control_prompt = oracle_control_plan(control_case, configured_model).body["messages"]
        missing_model = oracle_control_plan(control_case, configured_model)
        missing_model.body.pop("model")
        wrong_model = oracle_control_plan(control_case, configured_model)
        wrong_model.body["model"] = "different-model-sentinel"
        policy_in_body = oracle_control_plan(control_case, configured_model)
        policy_in_body.body["attempts"] = 1
        policy_in_body.body["retries"] = 0
        changed_tool = oracle_control_plan("buffered-tools", configured_model)
        changed_tool.body["tools"][0]["function"]["parameters"] = {
            "type": "object",
            "properties": {"invented": {"type": "string"}},
        }
        mismatched_streamed_tool = oracle_control_plan(
            "streamed-tools", configured_model
        )
        mismatched_streamed_tool.body["tools"][0]["function"][
            "name"
        ] = "different_tool"
        for name, case, plan in (
            ("request-profile-missing-model-control", control_case, missing_model),
            ("request-profile-wrong-model-control", control_case, wrong_model),
            ("request-profile-policy-in-body-control", control_case, policy_in_body),
            ("request-profile-tool-schema-control", "buffered-tools", changed_tool),
            (
                "request-profile-streamed-tool-match-control",
                "streamed-tools",
                mismatched_streamed_tool,
            ),
        ):
            expect_exact(
                name,
                {Problem(f"capture-request-profile:{case}")},
                oracle_request_profile(case, configured_model, plan, control_prompt),
                failures,
            )

    if failures:
        print("capture selftest RED — named contract failures remain:")
        for failure in failures:
            print(f"  RED {failure} (not-yet-implemented)")
        return 1
    print("capture selftest ok — every fixed candidate oracle passed")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", choices=CASES, help="one fixed request profile")
    parser.add_argument("--output-dir", type=pathlib.Path, help="new raw capture directory")
    parser.add_argument("--selftest", action="store_true", help="exercise RED contract fixtures")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    if args.case is None or args.output_dir is None:
        parser.error("--case and --output-dir are required unless --selftest is used")
    candidate_capture()
    print("[capture-not-implemented] live capture is unavailable", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
