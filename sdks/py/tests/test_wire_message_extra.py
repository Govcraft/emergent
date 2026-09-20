"""
Tests for a message envelope that carries a field this SDK does not know.

The engine may add a field to the envelope. A subscriber built against an older
SDK has to keep receiving messages when it does, as the TypeScript, Go and Rust
SDKs already do.
"""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

from emergent._client import BaseClient, PendingTopologyRequest, wire_message_from_push
from emergent._protocol import MessageType, encode_frame
from emergent.stream import MessageStream
from emergent.types import EmergentMessage, TopologyState, WireMessage

WIRE_MESSAGE: dict[str, Any] = {
    "id": "msg_74",
    "message_type": "py74.event",
    "source": "upstream",
    "timestamp_ms": 1700000000000,
    "payload": {"n": 1},
}

EXTRA_FIELDS = [
    pytest.param({"trace_id": "trc_01"}, id="string"),
    pytest.param({"priority": 3}, id="number"),
    pytest.param({"routing": {"hops": ["a", "b"]}}, id="object"),
    pytest.param({"ttl_ms": None}, id="null"),
    pytest.param({"trace_id": "trc_01", "priority": 3}, id="two-fields"),
]


def push_frame(message: dict[str, Any]) -> bytes:
    return encode_frame(
        MessageType.PUSH,
        {
            "notification_id": "ntf_74",
            "message_type": message["message_type"],
            "timestamp_ms": 1700000000001,
            "payload": message,
        },
    )


class TestWireMessageFromPush:
    @pytest.mark.parametrize("extra", EXTRA_FIELDS)
    def test_an_unknown_field_is_ignored(self, extra: dict[str, Any]) -> None:
        wire = wire_message_from_push({**WIRE_MESSAGE, **extra})

        assert wire == WireMessage.model_validate(WIRE_MESSAGE)

    def test_a_known_field_of_the_wrong_type_is_still_refused(self) -> None:
        assert wire_message_from_push({**WIRE_MESSAGE, "id": 7, "trace_id": "trc_01"}) is None


class TestDelivery:
    @pytest.mark.parametrize("extra", EXTRA_FIELDS)
    def test_a_message_with_an_unknown_field_reaches_the_subscriber(
        self, extra: dict[str, Any]
    ) -> None:
        client = BaseClient(name="py74", primitive_kind="Sink", timeout=5.0)
        stream = MessageStream()
        client._message_stream = stream

        client._read_buffer.extend(push_frame({**WIRE_MESSAGE, **extra}))
        client._process_frames()

        message = stream.try_next()
        assert message == EmergentMessage.from_wire(WireMessage.model_validate(WIRE_MESSAGE))

    async def test_a_topology_reply_with_an_unknown_field_is_answered(self) -> None:
        client = BaseClient(name="py74", primitive_kind="Sink", timeout=5.0)
        future: asyncio.Future[TopologyState] = asyncio.get_running_loop().create_future()
        client._pending_topology_requests["cor_74"] = PendingTopologyRequest(future=future)

        client._read_buffer.extend(
            push_frame(
                {
                    **WIRE_MESSAGE,
                    "message_type": "system.response.topology",
                    "correlation_id": "cor_74",
                    "payload": {"primitives": []},
                    "trace_id": "trc_01",
                }
            )
        )
        client._process_frames()

        assert await asyncio.wait_for(future, timeout=1) == TopologyState(primitives=())
