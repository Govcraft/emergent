"""Tests for EmergentMessage.unwrap_stdout and has_stdout_payload."""

import json

from emergent.types import EmergentMessage


def _make_message(payload: object) -> EmergentMessage:
    """Helper to build a minimal EmergentMessage with the given payload."""
    return EmergentMessage(
        id="msg_001",
        message_type="exec.output",
        source="exec_source",
        timestamp_ms=1700000000000,
        payload=payload,
    )


class TestHasStdoutPayload:
    """Tests for EmergentMessage.has_stdout_payload."""

    def test_with_stdout_string(self) -> None:
        """Payload with a stdout string key returns True."""
        msg = _make_message({"stdout": "hello world"})
        assert msg.has_stdout_payload() is True

    def test_without_stdout_key(self) -> None:
        """Payload dict missing the stdout key returns False."""
        msg = _make_message({"stderr": "error"})
        assert msg.has_stdout_payload() is False

    def test_stdout_not_string(self) -> None:
        """Payload with a non-string stdout value returns False."""
        msg = _make_message({"stdout": 42})
        assert msg.has_stdout_payload() is False

    def test_payload_is_none(self) -> None:
        """None payload returns False."""
        msg = _make_message(None)
        assert msg.has_stdout_payload() is False

    def test_payload_is_string(self) -> None:
        """String payload (not a dict) returns False."""
        msg = _make_message("raw string")
        assert msg.has_stdout_payload() is False

    def test_payload_is_list(self) -> None:
        """List payload returns False."""
        msg = _make_message([1, 2, 3])
        assert msg.has_stdout_payload() is False


class TestUnwrapStdoutJson:
    """Tests for unwrap_stdout when stdout contains valid JSON."""

    def test_json_object(self) -> None:
        """stdout holding a JSON object is parsed into a dict payload."""
        inner = {"temperature": 72.5, "unit": "F"}
        msg = _make_message({"stdout": json.dumps(inner)})

        result = msg.unwrap_stdout()

        assert result is not msg  # new instance
        assert result.payload == inner
        # envelope fields unchanged
        assert result.id == msg.id
        assert result.message_type == msg.message_type
        assert result.source == msg.source
        assert result.timestamp_ms == msg.timestamp_ms

    def test_json_array(self) -> None:
        """stdout holding a JSON array is parsed correctly."""
        inner = [1, 2, 3]
        msg = _make_message({"stdout": json.dumps(inner)})

        result = msg.unwrap_stdout()
        assert result.payload == inner

    def test_json_scalar(self) -> None:
        """stdout holding a JSON scalar (number) is parsed correctly."""
        msg = _make_message({"stdout": "42"})

        result = msg.unwrap_stdout()
        assert result.payload == 42


class TestUnwrapStdoutPlainText:
    """Tests for unwrap_stdout when stdout is non-JSON text."""

    def test_plain_text(self) -> None:
        """Non-JSON stdout is returned as a raw string payload."""
        msg = _make_message({"stdout": "hello world"})

        result = msg.unwrap_stdout()

        assert result is not msg
        assert result.payload == "hello world"

    def test_partial_json(self) -> None:
        """Malformed JSON falls back to the raw string."""
        msg = _make_message({"stdout": '{"incomplete": '})

        result = msg.unwrap_stdout()
        assert result.payload == '{"incomplete": '


class TestUnwrapStdoutNoStdoutField:
    """Tests for unwrap_stdout when the payload lacks a stdout field."""

    def test_no_stdout_key(self) -> None:
        """Returns the original message unchanged when stdout key is absent."""
        msg = _make_message({"stderr": "some error"})

        result = msg.unwrap_stdout()
        assert result is msg

    def test_none_payload(self) -> None:
        """Returns the original message unchanged when payload is None."""
        msg = _make_message(None)

        result = msg.unwrap_stdout()
        assert result is msg

    def test_string_payload(self) -> None:
        """Returns the original message unchanged when payload is a plain string."""
        msg = _make_message("just a string")

        result = msg.unwrap_stdout()
        assert result is msg
