"""Tests for the shutdown kind the SDK reads from a push notification."""

from typing import Any

import pytest

from emergent._client import BaseClient, extract_shutdown_kind
from emergent._protocol import MessageType
from emergent.stream import MessageStream


def engine_shutdown_envelope(kind: str) -> dict[str, Any]:
    """Build a ``system.shutdown`` payload exactly as the engine sends it.

    The engine forwards the whole serialized message as the notification
    payload, so the targeted kind sits one level in.
    """
    return {
        "id": "msg_01m2xskqtyffgve4n1yw9vr8kz",
        "message_type": "system.shutdown",
        "source": "emergent",
        "timestamp_ms": 1758318000000,
        "payload": {"kind": kind},
    }


def push_frame(message_type: str, payload: Any) -> dict[str, Any]:
    """Build the frame body of a push notification."""
    return {
        "notification_id": "push_01m2xskqtyffgve4n1yw9vr8kz",
        "message_type": message_type,
        "payload": payload,
        "source_actor": "emergent",
        "timestamp_ms": 1758318000000,
    }


def tick_envelope() -> dict[str, Any]:
    """Build an ordinary domain message envelope."""
    return {
        "id": "msg_01m2xt2bjje7gtk69d6b7zqcdp",
        "message_type": "timer.tick",
        "source": "ticker",
        "timestamp_ms": 1758318001000,
        "payload": {"sequence": 1},
    }


class TestExtractShutdownKind:
    """Tests for extract_shutdown_kind."""

    def test_reads_the_engine_envelope(self) -> None:
        assert extract_shutdown_kind(engine_shutdown_envelope("sink")) == "sink"

    def test_reads_a_bare_kind_object(self) -> None:
        assert extract_shutdown_kind({"kind": "handler"}) == "handler"

    def test_lowercases_the_kind(self) -> None:
        assert extract_shutdown_kind(engine_shutdown_envelope("SINK")) == "sink"

    def test_prefers_the_inner_envelope(self) -> None:
        payload = {"kind": "source", "payload": {"kind": "sink"}}
        assert extract_shutdown_kind(payload) == "sink"

    @pytest.mark.parametrize(
        "payload",
        [
            {},
            {"payload": {}},
            {"payload": {"kind": 7}},
            {"kind": None},
            "sink",
            None,
        ],
    )
    def test_reports_no_kind_when_the_payload_carries_none(self, payload: Any) -> None:
        assert extract_shutdown_kind(payload) is None


class TestShutdownClosesTheStream:
    """The stream a sink reads must end on its own shutdown phase."""

    def make_sink(self) -> tuple[BaseClient, MessageStream]:
        client = BaseClient(name="printer", primitive_kind="Sink")
        stream = MessageStream()
        client._message_stream = stream
        return client, stream

    def test_matching_shutdown_closes_the_stream(self) -> None:
        client, stream = self.make_sink()

        client._handle_frame(
            MessageType.PUSH,
            push_frame("system.shutdown", engine_shutdown_envelope("sink")),
        )

        assert stream.closed is True
        assert client._message_stream is None

    def test_shutdown_for_another_kind_leaves_the_stream_open(self) -> None:
        client, stream = self.make_sink()

        for kind in ("source", "handler"):
            client._handle_frame(
                MessageType.PUSH,
                push_frame("system.shutdown", engine_shutdown_envelope(kind)),
            )

        assert stream.closed is False
        assert client._message_stream is stream

        client._handle_frame(MessageType.PUSH, push_frame("timer.tick", tick_envelope()))

        message = stream.try_next()
        assert message is not None
        assert message.message_type == "timer.tick"
