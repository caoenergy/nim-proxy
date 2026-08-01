#!/usr/bin/env python3
"""RED contract harness for deterministic, secret-free NIM evidence fixtures.

The candidate API intentionally returns an identity-shaped result.  The
selftest's independent privacy and fidelity oracles therefore fail until Task
14 replaces only that candidate implementation with a real sanitizer.
"""

import argparse
import base64
import contextlib
import io
import json
from pathlib import Path
import tempfile


def raw_envelope(body, **forbidden):
    envelope = {
        "format": "nim-capture-raw-v1",
        "case": "streamed-basic",
        "capture_date": "2026-08-01",
        "transport": "sse",
        "status": 200,
        "content_type": "text/event-stream",
        "truncated": False,
        "body_b64": base64.b64encode(body).decode("ascii"),
    }
    envelope.update(forbidden)
    return envelope


def write_raw(path, envelope):
    path.write_text(json.dumps(envelope, sort_keys=True), encoding="utf-8")


def candidate_sanitize(raw_path):
    """Stable Task 14 candidate API; deliberately noncompliant RED behavior."""
    return {"filename": raw_path.name, "fixture": json.loads(raw_path.read_text("utf-8"))}


def body_bytes(value):
    try:
        return base64.b64decode(value, validate=True)
    except (TypeError, ValueError):
        return None


def has_sentinel(value, sentinel):
    """Inspect candidate structures recursively, decoding opaque byte channels."""
    if isinstance(value, dict):
        return any(has_sentinel(key, sentinel) or has_sentinel(item, sentinel) for key, item in value.items())
    if isinstance(value, list):
        return any(has_sentinel(item, sentinel) for item in value)
    if isinstance(value, bytes):
        return sentinel.encode() in value
    if isinstance(value, str):
        return sentinel in value or sentinel.encode() in (body_bytes(value) or b"")
    return False


def candidate_body(candidate):
    body = candidate.get("fixture", {}).get("body")
    return body.encode("utf-8") if isinstance(body, str) else None


def json_pairs(body):
    try:
        return json.loads(body, object_pairs_hook=lambda pairs: ("object", pairs))
    except (TypeError, UnicodeDecodeError, json.JSONDecodeError):
        return None


def pairs_get(value, key):
    if not (isinstance(value, tuple) and value[0] == "object"):
        return []
    return [item for name, item in value[1] if name == key]


def json_type(value):
    if value is None:
        return "null"
    if isinstance(value, tuple):
        return "object"
    if isinstance(value, list):
        return "array"
    if isinstance(value, bool):
        return "bool"
    if isinstance(value, str):
        return "string"
    if isinstance(value, (int, float)):
        return "number"
    return "absent"


def number_class(value):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return json_type(value)
    return ("negative" if value < 0 else "zero" if value == 0 else "positive",
            "integer" if isinstance(value, int) else "fractional")


KNOWN_FINISH = {"content_filter", "function_call", "length", "stop", "tool_calls"}


def protected_json(value, key=None, numbers=None, identifiers=None):
    """Content-free JSON shape, with only observer-relevant equivalence classes."""
    numbers = {} if numbers is None else numbers
    identifiers = {} if identifiers is None else identifiers
    if isinstance(value, tuple) and value[0] == "object":
        return ("object", tuple(
            (name, protected_json(item, name, numbers, identifiers))
            for name, item in value[1]
        ))
    if isinstance(value, list):
        return ("array", tuple(protected_json(item, None, numbers, identifiers) for item in value))
    if value is None or isinstance(value, bool):
        return json_type(value)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        signature = (number_class(value), repr(value))
        reference = numbers.setdefault(signature, len(numbers))
        return ("number", number_class(value), reference)
    if isinstance(value, str):
        if key == "finish_reason":
            return ("finish", value if value in KNOWN_FINISH else "__other__")
        if key in {"id", "tool_call_id"}:
            reference = identifiers.setdefault(value, len(identifiers))
            return ("identifier", reference)
        return ("redacted",)
    return ("absent",)


def contains_key(value, key):
    if isinstance(value, tuple) and value[0] == "object":
        return any(name == key or contains_key(item, key) for name, item in value[1])
    if isinstance(value, list):
        return any(contains_key(item, key) for item in value)
    return False


def sse_signature(body):
    if body is None:
        return None
    newline = "crlf" if b"\r\n" in body else "lf"
    normalized = body.replace(b"\r\n", b"\n")
    terminated = normalized.endswith(b"\n\n")
    segments = normalized.split(b"\n\n")
    numbers, identifiers = {}, {}
    parsed = []
    for segment in segments:
        lines = segment.split(b"\n") if segment else []
        kinds = [line.partition(b":")[0].decode("ascii", "replace") for line in lines]
        data = [line.partition(b":")[2].lstrip() for line in lines if line.startswith(b"data:")]
        joined = b"\n".join(data)
        parsed_json = json_pairs(joined)
        parsed.append((
            tuple(kinds),
            len(data),
            joined == b"[DONE]",
            protected_json(parsed_json, numbers=numbers, identifiers=identifiers) if parsed_json is not None else None,
            segment == b"",
        ))
    return (newline, terminated, tuple(parsed))


def protected_signature(field, body):
    if field.startswith("sse-") or field == "streamed-tool-calls":
        signature = sse_signature(body)
        if signature is None:
            return None
        newline, terminated, events = signature
        if field == "sse-event-order":
            return tuple(event[:4] for event in events)
        if field == "sse-line-kind":
            return tuple(event[0] for event in events)
        if field == "sse-data-count":
            return tuple(event[1] for event in events)
        if field == "sse-boundary":
            return (tuple(event[4] for event in events), terminated)
        if field == "sse-newline":
            return newline
        if field == "sse-done":
            return tuple(event[2] for event in events)
        if field == "sse-malformed":
            return tuple(event[3] is None for event in events)
        if field == "sse-truncated":
            return terminated
        return tuple(event[3] is not None and contains_key(event[3], "tool_calls") for event in events)

    root = json_pairs(body)
    if root is None:
        return None
    usage = pairs_get(root, "usage")
    choices = pairs_get(root, "choices")
    if field == "usage-presence":
        return bool(usage)
    if field == "usage-type":
        return json_type(usage[0]) if usage else "absent"
    if field == "usage-field-order":
        return tuple(name for name, _ in usage[0][1]) if usage and isinstance(usage[0], tuple) else ()
    if field in {"usage-equality", "usage-conflict"}:
        values = pairs_get(usage[0], "total_tokens") if usage else []
        return (len(values), len({repr(value) for value in values}) == 1)
    if field == "usage-number-class":
        values = pairs_get(usage[0], "completion_tokens") if usage else []
        return number_class(values[0]) if values else "absent"
    if field == "choice-order":
        return tuple(pairs_get(choice, "index")[0] for choice in choices[0] if pairs_get(choice, "index")) if choices else ()
    if field == "finish-reason":
        reasons = pairs_get(choices[0][0], "finish_reason") if choices and choices[0] else []
        return reasons[0] if reasons and reasons[0] in KNOWN_FINISH else "__other__" if reasons else "absent"
    if field == "buffered-tool-calls":
        message = pairs_get(choices[0][0], "message") if choices and choices[0] else []
        tools = pairs_get(message[0], "tool_calls") if message else []
        return json_type(tools[0]) if tools else "absent"
    if field == "error":
        error = pairs_get(root, "error")
        return json_type(error[0]) if error else "absent"
    raise AssertionError(field)


def shape_body(field):
    json_bodies = {
        "usage-presence": b'{"choices":[{"index":0,"finish_reason":"stop"}]}',
        "usage-type": b'{"usage":[],"choices":[]}',
        "usage-field-order": b'{"usage":{"completion_tokens":2,"prompt_tokens":1}}',
        "usage-equality": b'{"usage":{"total_tokens":3,"total_tokens":3}}',
        "usage-conflict": b'{"usage":{"total_tokens":3,"total_tokens":4}}',
        "usage-number-class": b'{"usage":{"completion_tokens":-1.5}}',
        "choice-order": b'{"choices":[{"index":1},{"index":0}]}',
        "finish-reason": b'{"choices":[{"index":0,"finish_reason":"provider-specific"}]}',
        "buffered-tool-calls": b'{"choices":[{"message":{"tool_calls":null}}]}',
        "error": b'{"error":{"code":"synthetic"}}',
    }
    if field in json_bodies:
        return json_bodies[field]
    return {
        "streamed-tool-calls": b'data: {"choices":[{"delta":{"tool_calls":[{}]}}]}\n\n',
        "sse-event-order": b'data: {"choices":[{"index":1}]}\n\ndata: {"choices":[{"index":0}]}\n\n',
        "sse-line-kind": b'retry: 10\ndata: {}\n\n',
        "sse-data-count": b'data: {"usage":\ndata: {"prompt_tokens":1}}\n\n',
        "sse-boundary": b'data: {}\ndata: [DONE]\n',
        "sse-newline": b'data: {}\r\n\r\n',
        "sse-done": b'data: {}\n\n',
        "sse-malformed": b'data: {not-json}\n\n',
        "sse-truncated": b'data: {"usage":{"prompt_tokens":1}}',
    }[field]


def secret_problems(candidate, channel, sentinel):
    fixture = candidate["fixture"]
    body = body_bytes(fixture.get("body_b64"))
    targets = {
        "envelope-authorization": fixture.get("authorization"),
        "envelope-request-headers": fixture.get("request_headers"),
        "envelope-request-body": fixture.get("request_body"),
        "envelope-url": fixture.get("url"),
        "envelope-model": fixture.get("model"),
        "envelope-provider-headers": fixture.get("provider_headers"),
        "envelope-timestamp": fixture.get("captured_at"),
        "nested-json-string": body,
        "sse-comment": body,
        "sse-data": body,
        "sse-id": body,
        "sse-prose": body,
        "opaque-id": body,
        "filename": candidate["filename"],
    }
    if not has_sentinel(targets[channel], sentinel):
        return []
    return [f"capture-secret-leak:{channel}"]


def shape_problems(candidate, field, raw_body):
    expected = protected_signature(field, raw_body)
    actual = protected_signature(field, candidate_body(candidate))
    if actual == expected:
        return []
    return [f"capture-shape-drift:{field}"]


def require_control(actual, expected):
    if actual != [expected]:
        raise AssertionError(f"oracle control {expected}: got {actual}")


def run_cases(sentinel):
    candidate_checks = []
    controls = 0
    private_event = b'data: {"choices":[{"index":0,"delta":{"content":"private-text"}}]}\n\n'
    redacted_event = b'data: {"choices":[{"index":0,"delta":{"content":"<redacted>"}}]}\n\n'
    if protected_signature("sse-event-order", private_event) != protected_signature("sse-event-order", redacted_event):
        raise AssertionError("capture-shape-drift:sse-redaction-control")
    one_boundary = b"data: {}\n\n"
    added_boundary = b"data: {}\n\n\n\n"
    if protected_signature("sse-boundary", one_boundary) == protected_signature("sse-boundary", added_boundary):
        raise AssertionError("capture-shape-drift:sse-boundary-control")
    buffered = json.dumps({"id": f"opaque-{sentinel}", "choices": [{"message": {"content": f"nested {sentinel}"}}]}).encode()
    forbidden = {
        "authorization": f"Bearer {sentinel}", "request_headers": {"x-secret": sentinel},
        "request_body": {"model": sentinel}, "url": f"https://{sentinel}.invalid/",
        "model": sentinel, "provider_headers": {"x-provider-secret": sentinel}, "captured_at": sentinel,
    }
    secret_cases = (
        ("envelope-authorization", "authorization"), ("envelope-request-headers", "request_headers"),
        ("envelope-request-body", "request_body"), ("envelope-url", "url"),
        ("envelope-model", "model"), ("envelope-provider-headers", "provider_headers"),
        ("envelope-timestamp", "captured_at"), ("nested-json-string", None),
        ("sse-comment", None), ("sse-data", None), ("sse-id", None),
        ("sse-prose", None), ("opaque-id", None), ("filename", None),
    )
    shape_fields = (
        "usage-presence", "usage-type", "usage-field-order", "usage-equality", "usage-conflict",
        "usage-number-class", "choice-order", "finish-reason", "buffered-tool-calls",
        "streamed-tool-calls", "error", "sse-event-order", "sse-line-kind", "sse-data-count",
        "sse-boundary", "sse-newline", "sse-done", "sse-malformed", "sse-truncated",
    )
    with tempfile.TemporaryDirectory(prefix="nim-sanitize-red-") as directory:
        root, raw = Path(directory), Path(directory) / "raw.json"
        original = raw_envelope(b"{}")
        for channel, envelope_field in secret_cases:
            write_raw(raw, original)  # restore before every negative fixture
            name = raw
            if envelope_field:
                envelope = raw_envelope(b"{}"); envelope[envelope_field] = forbidden[envelope_field]
            elif channel == "nested-json-string":
                envelope = raw_envelope(buffered)
            elif channel == "sse-comment":
                envelope = raw_envelope(f": {sentinel}\ndata: {{}}\n\n".encode())
            elif channel == "sse-data":
                envelope = raw_envelope(f"data: {{\"content\":\"{sentinel}\"}}\n\n".encode())
            elif channel == "sse-id":
                envelope = raw_envelope(f"id: {sentinel}\ndata: {{}}\n\n".encode())
            elif channel == "sse-prose":
                envelope = raw_envelope(f"data: prose {sentinel}\n\n".encode())
            elif channel == "opaque-id":
                envelope = raw_envelope(f"{{\"id\":\"opaque-{sentinel}\"}}".encode())
            else:
                name = root / f"fixture-{sentinel}.json"; envelope = original
            write_raw(name, envelope)
            expected = f"capture-secret-leak:{channel}"
            control = {"filename": name.name, "fixture": envelope}
            require_control(secret_problems(control, channel, sentinel), expected)
            controls += 1
            candidate_checks.extend(secret_problems(candidate_sanitize(name), channel, sentinel))
            if name != raw:
                name.unlink()
        for field in shape_fields:
            write_raw(raw, original)  # restore before every negative fixture
            body = shape_body(field)
            write_raw(raw, raw_envelope(body))
            expected = f"capture-shape-drift:{field}"
            control = {"filename": "broken.json", "fixture": raw_envelope(body)}
            require_control(shape_problems(control, field, body), expected)
            controls += 1
            candidate_checks.extend(shape_problems(candidate_sanitize(raw), field, body))
    return candidate_checks, controls


def selftest():
    sentinel = "SENTINEL-RAW-SECRET-9f2c"
    stdout, stderr = io.StringIO(), io.StringIO()
    exception_text = ""
    try:
        with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
            checks, controls = run_cases(sentinel)
    except Exception as error:  # an implementation exception is itself inspected for leaks
        checks = []
        controls = 0
        exception_text = str(error)
    captured = stdout.getvalue() + stderr.getvalue() + exception_text
    if sentinel in captured:
        raise AssertionError("capture-secret-leak:diagnostics")
    expected_count = 14 + 19
    if controls != expected_count:
        raise AssertionError("capture-shape-drift:selftest-control-count")
    for check in checks:
        print(f"RED {check}")
    if checks:
        print(f"selftest RED — {len(checks)} candidate violations; {controls} oracle controls observed")
        return 1
    print(f"selftest ok — {controls} oracle controls observed; candidate compliant")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--selftest", action="store_true", help="run deliberate Task 14 RED cases")
    args = parser.parse_args(argv)
    if args.selftest:
        return selftest()
    parser.error("only --selftest is available until sanitizer implementation")


if __name__ == "__main__":
    raise SystemExit(main())
