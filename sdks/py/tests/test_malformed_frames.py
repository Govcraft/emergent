"""Tests for frames whose body is malformed.

One bad frame must never end the read loop or close the subscriber stream. It
is logged and skipped, and the frames behind it in the same chunk are still
handled.
"""

import asyncio
import json
import struct
from typing import Any

import pytest

from emergent._client import (
    BaseClient,
    PendingTopologyRequest,
    push_from_frame,
    subscribes_from_payload,
    topology_from_payload,
    wire_message_from_push,
)
from emergent._protocol import (
    HEADER_SIZE,
    MAX_FRAME_SIZE,
    BadFrameBody,
    BadFraming,
    DecodedFrame,
    Format,
    MessageType,
    decode_payload,
    encode_frame,
    next_frame,
    try_decode_frame,
)
from emergent.errors import ProtocolError
from emergent.stream import MessageStream
from emergent.types import TopologyPrimitive, TopologyState

WIRE_MESSAGE: dict[str, Any] = {
    "id": "msg_01",
    "message_type": "py64.event",
    "source": "upstream",
    "timestamp_ms": 1700000000000,
    "payload": {"n": 1},
}

NOTIFICATION: dict[str, Any] = {
    "notification_id": "ntf_01",
    "message_type": "py64.event",
    "timestamp_ms": 1700000000001,
    "payload": WIRE_MESSAGE,
}

GOOD_FRAME = encode_frame(MessageType.PUSH, NOTIFICATION)


def raw_frame(msg_type: int, format_raw: int, body: bytes) -> bytes:
    """A frame with the given body bytes, whatever they hold."""
    return struct.pack(">IBBB", len(body), 0x02, msg_type, format_raw) + body


TRUNCATED_MSGPACK = GOOD_FRAME[HEADER_SIZE:-5]
TRUNCATED_JSON = json.dumps(NOTIFICATION).encode("utf-8")[:-5]

UNDECODABLE_BODIES = [
    pytest.param(Format.MSGPACK, TRUNCATED_MSGPACK, id="truncated-msgpack"),
    pytest.param(Format.MSGPACK, TRUNCATED_MSGPACK[:1], id="msgpack-cut-after-one-byte"),
    pytest.param(Format.MSGPACK, GOOD_FRAME[HEADER_SIZE:] + b"\x01", id="msgpack-extra-data"),
    pytest.param(Format.JSON, TRUNCATED_JSON, id="truncated-json"),
    pytest.param(Format.JSON, b"nope", id="not-json"),
    pytest.param(Format.JSON, b'{"a":"\xff\xfe"}', id="json-invalid-utf8"),
    pytest.param(0x09, b"{}", id="unknown-format"),
]


class TestDecodePayload:
    @pytest.mark.parametrize(("format_raw", "body"), UNDECODABLE_BODIES)
    def test_a_body_that_does_not_decode_raises_a_protocol_error(
        self, format_raw: int, body: bytes
    ) -> None:
        with pytest.raises(ProtocolError):
            decode_payload(body, format_raw)


class TestNextFrame:
    @pytest.mark.parametrize("length", [0, 3, HEADER_SIZE, len(GOOD_FRAME) - 1])
    def test_waits_for_a_whole_frame(self, length: int) -> None:
        assert next_frame(GOOD_FRAME[:length]) is None

    @pytest.mark.parametrize("format_", [Format.JSON, Format.MSGPACK])
    def test_decodes_a_whole_frame(self, format_: Format) -> None:
        frame = encode_frame(MessageType.PUSH, NOTIFICATION, format_)
        assert next_frame(frame + b"\x01\x02\x03") == DecodedFrame(
            msg_type=MessageType.PUSH,
            format=format_,
            payload=NOTIFICATION,
            bytes_consumed=len(frame),
            raw_msg_type=MessageType.PUSH,
        )

    @pytest.mark.parametrize(("format_raw", "body"), UNDECODABLE_BODIES)
    def test_reports_a_bad_body_with_the_length_to_skip(self, format_raw: int, body: bytes) -> None:
        step = next_frame(raw_frame(MessageType.PUSH, format_raw, body) + GOOD_FRAME)
        assert isinstance(step, BadFrameBody)
        assert step.raw_msg_type == MessageType.PUSH
        assert step.bytes_consumed == HEADER_SIZE + len(body)

    @pytest.mark.parametrize(
        "header",
        [
            pytest.param(struct.pack(">IBBB", MAX_FRAME_SIZE + 1, 2, 5, 2), id="too-large"),
            pytest.param(struct.pack(">IBBB", 0, 1, 5, 2), id="wrong-version"),
        ],
    )
    def test_reports_a_header_it_cannot_trust(self, header: bytes) -> None:
        assert isinstance(next_frame(header), BadFraming)
        with pytest.raises(ProtocolError):
            try_decode_frame(header)

    def test_try_decode_frame_raises_a_protocol_error_for_a_bad_body(self) -> None:
        with pytest.raises(ProtocolError):
            try_decode_frame(raw_frame(MessageType.PUSH, Format.MSGPACK, TRUNCATED_MSGPACK))


class TestPushFromFrame:
    def test_reads_a_notification(self) -> None:
        notification = push_from_frame(NOTIFICATION)
        assert notification is not None
        assert notification.message_type == "py64.event"
        assert notification.payload == WIRE_MESSAGE

    @pytest.mark.parametrize(
        "body",
        [
            None,
            "text",
            7,
            [],
            [NOTIFICATION],
            {},
            {"payload": WIRE_MESSAGE},
            {**NOTIFICATION, "message_type": 7},
            {**NOTIFICATION, "message_type": None},
            {**NOTIFICATION, "notification_id": ["ntf_01"]},
        ],
    )
    def test_reads_nothing_from_the_wrong_shape(self, body: Any) -> None:
        assert push_from_frame(body) is None


class TestWireMessageFromPush:
    def test_reads_a_whole_message(self) -> None:
        wire = wire_message_from_push(
            {**WIRE_MESSAGE, "correlation_id": "cor_1", "causation_id": None}
        )
        assert wire is not None
        assert wire.id == "msg_01"
        assert wire.correlation_id == "cor_1"
        assert wire.causation_id is None
        assert wire.payload == {"n": 1}

    @pytest.mark.parametrize(
        "body",
        [
            None,
            "text",
            [],
            {},
            {k: v for k, v in WIRE_MESSAGE.items() if k != "id"},
            {k: v for k, v in WIRE_MESSAGE.items() if k != "timestamp_ms"},
            {**WIRE_MESSAGE, "id": 1},
            {**WIRE_MESSAGE, "message_type": 2},
            {**WIRE_MESSAGE, "source": []},
            {**WIRE_MESSAGE, "timestamp_ms": "x"},
            {**WIRE_MESSAGE, "correlation_id": 9},
            {**WIRE_MESSAGE, "causation_id": {}},
        ],
    )
    def test_reads_nothing_from_the_wrong_shape(self, body: Any) -> None:
        assert wire_message_from_push(body) is None


TIMER = {"name": "timer", "kind": "source", "state": "running", "publishes": ["timer.tick"]}
TIMER_PRIMITIVE = TopologyPrimitive(
    name="timer", kind="source", state="running", publishes=("timer.tick",)
)


class TestTopologyFromPayload:
    @pytest.mark.parametrize(
        ("payload", "primitives"),
        [
            ({"primitives": [TIMER]}, (TIMER_PRIMITIVE,)),
            ({"primitives": [None, 7, "timer", TIMER]}, (TIMER_PRIMITIVE,)),
            ({"primitives": [{**TIMER, "state": "melting"}, TIMER]}, (TIMER_PRIMITIVE,)),
            ({"primitives": [{**TIMER, "publishes": None}, TIMER]}, (TIMER_PRIMITIVE,)),
            ({"primitives": [{}]}, (TopologyPrimitive(name="", kind="", state="stopped"),)),
            ({"primitives": "timer"}, ()),
            ({"primitives": None}, ()),
            ({}, ()),
            (None, ()),
            ("text", ()),
        ],
    )
    def test_drops_entries_of_the_wrong_shape(
        self, payload: Any, primitives: tuple[TopologyPrimitive, ...]
    ) -> None:
        assert topology_from_payload(payload) == TopologyState(primitives=primitives)


class TestSubscribesFromPayload:
    @pytest.mark.parametrize(
        ("payload", "subscribes"),
        [
            ({"subscribes": ["a.b", "c.*"]}, ["a.b", "c.*"]),
            ({"subscribes": ["a.b", 7, None, ["c.d"], "e.f"]}, ["a.b", "e.f"]),
            ({"subscribes": "a.b"}, []),
            ({"subscribes": None}, []),
            ({}, []),
            (None, []),
            (7, []),
        ],
    )
    def test_drops_entries_that_are_not_strings(self, payload: Any, subscribes: list[str]) -> None:
        assert subscribes_from_payload(payload) == subscribes


MALFORMED_FRAMES = [
    pytest.param(encode_frame(MessageType.PUSH, None), id="push-null-body"),
    pytest.param(encode_frame(MessageType.PUSH, [NOTIFICATION]), id="push-array-body"),
    pytest.param(
        encode_frame(MessageType.PUSH, {**NOTIFICATION, "message_type": 7}),
        id="push-numeric-message-type",
    ),
    pytest.param(
        encode_frame(MessageType.PUSH, {**NOTIFICATION, "payload": None}),
        id="push-null-message",
    ),
    pytest.param(
        encode_frame(
            MessageType.PUSH,
            {
                **NOTIFICATION,
                "payload": {"id": 1, "message_type": 2, "source": [], "timestamp_ms": "x"},
            },
        ),
        id="push-wrong-typed-message-fields",
    ),
    pytest.param(
        encode_frame(
            MessageType.PUSH,
            {**NOTIFICATION, "message_type": "system.response.topology", "payload": None},
        ),
        id="topology-response-null-message",
    ),
    pytest.param(
        encode_frame(
            MessageType.PUSH,
            {**NOTIFICATION, "message_type": "system.response.subscriptions", "payload": None},
        ),
        id="subscriptions-response-null-message",
    ),
    pytest.param(
        raw_frame(MessageType.PUSH, Format.MSGPACK, TRUNCATED_MSGPACK),
        id="push-truncated-msgpack",
    ),
    pytest.param(
        raw_frame(MessageType.RESPONSE, Format.MSGPACK, TRUNCATED_MSGPACK),
        id="response-truncated-msgpack",
    ),
    pytest.param(
        raw_frame(MessageType.PUSH, Format.JSON, TRUNCATED_JSON), id="push-truncated-json"
    ),
    pytest.param(raw_frame(MessageType.PUSH, 0x09, b"{}"), id="push-unknown-format"),
]


def subscribed_client() -> tuple[BaseClient, MessageStream]:
    client = BaseClient(name="py64", primitive_kind="Sink", timeout=5.0)
    stream = MessageStream()
    client._message_stream = stream
    return client, stream


class TestTheReadLoopSurvives:
    @pytest.mark.parametrize("bad", MALFORMED_FRAMES)
    def test_a_good_frame_behind_a_malformed_one_is_still_delivered(self, bad: bytes) -> None:
        client, stream = subscribed_client()

        # One chunk, so a frame that stopped the loop would strand the good one.
        client._read_buffer.extend(bad + GOOD_FRAME)
        client._process_frames()

        assert not stream.closed
        message = stream.try_next()
        assert message is not None
        assert message.id == "msg_01"
        assert message.payload == {"n": 1}
        assert client._read_buffer == bytearray()

    def test_a_malformed_frame_split_across_chunks_is_skipped(self) -> None:
        client, stream = subscribed_client()
        chunk = raw_frame(MessageType.PUSH, Format.MSGPACK, TRUNCATED_MSGPACK) + GOOD_FRAME

        client._read_buffer.extend(chunk[:20])
        client._process_frames()
        assert stream.try_next() is None
        client._read_buffer.extend(chunk[20:])
        client._process_frames()

        message = stream.try_next()
        assert message is not None
        assert message.id == "msg_01"

    async def test_a_topology_query_survives_a_malformed_reply(self) -> None:
        client, _ = subscribed_client()
        future: asyncio.Future[TopologyState] = asyncio.get_running_loop().create_future()
        client._pending_topology_requests["cor_1"] = PendingTopologyRequest(future=future)

        def reply(payload: Any) -> bytes:
            return encode_frame(
                MessageType.PUSH,
                {**NOTIFICATION, "message_type": "system.response.topology", "payload": payload},
            )

        client._read_buffer.extend(
            reply(None)
            + reply({**WIRE_MESSAGE, "correlation_id": 9})
            + reply(
                {
                    **WIRE_MESSAGE,
                    "message_type": "system.response.topology",
                    "correlation_id": "cor_1",
                    "payload": {"primitives": [None, TIMER]},
                }
            )
        )
        client._process_frames()

        assert await future == TopologyState(primitives=(TIMER_PRIMITIVE,))
