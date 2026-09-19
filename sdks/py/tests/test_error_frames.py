"""Tests for how the client settles RESPONSE and ERROR frames.

The engine answers a failed request with an ERROR frame (0x03) whose body is
the same response object a RESPONSE frame carries. The bodies below were
captured from an engine on acton-reactive 9.3.0.
"""

import asyncio
import logging
from typing import Any

import pytest

from emergent._client import (
    DEFAULT_ERROR_TEXT,
    BaseClient,
    error_frame_text,
    response_from_frame,
)
from emergent._protocol import MessageType, encode_frame, try_decode_frame
from emergent.errors import ConnectionError, PublishError
from emergent.message import create_message

ACTOR_NOT_FOUND = {
    "correlation_id": "req_x",
    "success": False,
    "error": "Actor not found: no_such_actor",
    "error_code": "ACTOR_NOT_FOUND",
}

UNPARSEABLE_REQUEST = {
    "correlation_id": "unknown",
    "success": False,
    "error": "Parse error: Serialization error: missing field `correlation_id`",
}


class FakeWriter:
    """Stands in for the socket writer and keeps what the client sent."""

    def __init__(self) -> None:
        self.frames: list[bytes] = []

    def write(self, data: bytes) -> None:
        self.frames.append(data)

    async def drain(self) -> None:
        return None

    def sent(self, index: int = -1) -> tuple[MessageType | None, Any]:
        decoded = try_decode_frame(self.frames[index])
        assert decoded is not None
        return decoded.msg_type, decoded.payload


def connected_client() -> tuple[BaseClient, FakeWriter]:
    client = BaseClient(name="py54", primitive_kind="Source", timeout=5.0)
    writer = FakeWriter()
    client._writer = writer  # type: ignore[assignment]
    return client, writer


async def wait_for_frame(writer: FakeWriter, count: int = 1) -> None:
    for _ in range(100):
        if len(writer.frames) >= count:
            return
        await asyncio.sleep(0)
    raise AssertionError("the client never wrote a frame")


class TestResponseFromFrame:
    """The pure reading of a RESPONSE or ERROR frame body."""

    def test_error_frame_reads_as_a_failure_with_its_text(self) -> None:
        response = response_from_frame(MessageType.ERROR, ACTOR_NOT_FOUND)

        assert response is not None
        assert response.correlation_id == "req_x"
        assert response.success is False
        assert response.error == "Actor not found: no_such_actor"
        assert response.error_code == "ACTOR_NOT_FOUND"

    def test_error_frame_is_a_failure_whatever_its_success_field_says(self) -> None:
        response = response_from_frame(
            MessageType.ERROR, {"correlation_id": "req_x", "success": True}
        )

        assert response is not None
        assert response.success is False
        assert response.error == DEFAULT_ERROR_TEXT

    def test_response_frame_is_read_as_sent(self) -> None:
        body = {"correlation_id": "req_y", "success": True, "payload": {"ok": 1}}
        response = response_from_frame(MessageType.RESPONSE, body)

        assert response is not None
        assert response.success is True
        assert response.payload == {"ok": 1}
        assert response.error is None

    def test_fields_beside_the_shared_ones_are_kept(self) -> None:
        body = {"correlation_id": "sub_1", "success": True, "subscribed_types": ["a.b"]}
        response = response_from_frame(MessageType.RESPONSE, body)

        assert response is not None
        assert response.model_dump()["subscribed_types"] == ["a.b"]

    @pytest.mark.parametrize(
        "payload",
        [
            None,
            "text",
            [],
            {},
            {"success": False, "error": "no id"},
            {"correlation_id": 7, "success": False},
            {"correlation_id": "req_z"},
        ],
    )
    def test_body_that_cannot_be_matched_reads_as_none(self, payload: Any) -> None:
        assert response_from_frame(MessageType.ERROR, payload) is None
        assert response_from_frame(MessageType.RESPONSE, payload) is None

    @pytest.mark.parametrize(
        "msg_type",
        [m for m in MessageType if m not in (MessageType.RESPONSE, MessageType.ERROR)],
    )
    def test_other_frame_types_carry_no_response(self, msg_type: MessageType) -> None:
        assert response_from_frame(msg_type, ACTOR_NOT_FOUND) is None


class TestErrorFrameText:
    """The log line for an ERROR frame nothing was waiting on."""

    def test_text_and_code(self) -> None:
        assert error_frame_text(ACTOR_NOT_FOUND) == (
            "Actor not found: no_such_actor (ACTOR_NOT_FOUND)"
        )

    def test_text_alone(self) -> None:
        assert error_frame_text(UNPARSEABLE_REQUEST) == UNPARSEABLE_REQUEST["error"]

    @pytest.mark.parametrize("payload", [None, [], {}, {"error": ""}, {"error": 3}])
    def test_falls_back_when_there_is_no_text(self, payload: Any) -> None:
        assert error_frame_text(payload) == DEFAULT_ERROR_TEXT


class TestErrorFrameSettlesTheRequest:
    """An ERROR frame must fail the request that is waiting on it."""

    async def test_publish_ack_raises_with_the_engine_error_text(self) -> None:
        client, writer = connected_client()
        message = create_message("py54.event").payload({"n": 1}).build()

        task = asyncio.create_task(client._publish_ack(message))
        await wait_for_frame(writer)
        msg_type, request = writer.sent()
        assert msg_type is MessageType.REQUEST

        error = {**ACTOR_NOT_FOUND, "correlation_id": request["correlation_id"]}
        client._read_buffer.extend(encode_frame(MessageType.ERROR, error))
        client._process_frames()

        with pytest.raises(PublishError, match="Actor not found: no_such_actor"):
            await asyncio.wait_for(task, timeout=1.0)
        assert client._pending_requests == {}

    async def test_subscribe_raises_with_the_engine_error_text(self) -> None:
        client, writer = connected_client()

        task = asyncio.create_task(client._subscribe(["py54.event"]))
        await wait_for_frame(writer)
        msg_type, request = writer.sent()
        assert msg_type is MessageType.SUBSCRIBE

        error = {
            "correlation_id": request["correlation_id"],
            "success": False,
            "error": "Subscription limit reached",
            "subscribed_types": [],
        }
        client._handle_frame(MessageType.ERROR, error)

        with pytest.raises(ConnectionError, match="Subscription limit reached"):
            await asyncio.wait_for(task, timeout=1.0)

    async def test_the_timeout_timer_is_cancelled(self) -> None:
        client, writer = connected_client()
        message = create_message("py54.event").build()

        task = asyncio.create_task(client._publish_ack(message))
        await wait_for_frame(writer)
        _, request = writer.sent()
        pending = client._pending_requests[request["correlation_id"]]
        assert pending.timer is not None

        client._handle_frame(
            MessageType.ERROR, {**ACTOR_NOT_FOUND, "correlation_id": request["correlation_id"]}
        )

        assert pending.timer.cancelled()
        with pytest.raises(PublishError):
            await task

    async def test_error_for_another_request_leaves_this_one_waiting(
        self, caplog: pytest.LogCaptureFixture
    ) -> None:
        client, writer = connected_client()
        message = create_message("py54.event").build()

        task = asyncio.create_task(client._publish_ack(message))
        await wait_for_frame(writer)

        with caplog.at_level(logging.ERROR, logger="emergent"):
            client._handle_frame(MessageType.ERROR, UNPARSEABLE_REQUEST)

        assert not task.done()
        assert len(client._pending_requests) == 1
        assert "matched no pending request" in caplog.text
        assert "missing field `correlation_id`" in caplog.text

        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task


class TestFramesTheClientIgnores:
    """Frames that must neither settle a request nor stop the read loop."""

    async def test_heartbeat_stream_and_unknown_frames_are_passed_over(
        self, caplog: pytest.LogCaptureFixture
    ) -> None:
        client, writer = connected_client()
        message = create_message("py54.event").build()

        task = asyncio.create_task(client._publish_ack(message))
        await wait_for_frame(writer)
        _, request = writer.sent()

        heartbeat = b"\x00\x00\x00\x00\x02\x04\x01"
        unknown = b"\x00\x00\x00\x02\x02\x0c\x01{}"
        stream = encode_frame(
            MessageType.STREAM,
            {"correlation_id": "str_1", "sequence": 0, "is_final": True},
        )
        ok = encode_frame(
            MessageType.RESPONSE,
            {"correlation_id": request["correlation_id"], "success": True},
        )

        with caplog.at_level(logging.WARNING, logger="emergent"):
            client._read_buffer.extend(heartbeat + unknown + stream + ok)
            client._process_frames()

        await asyncio.wait_for(task, timeout=1.0)
        assert client._read_buffer == bytearray()
        assert "unknown type" in caplog.text
        assert "0x0c" in caplog.text

    def test_malformed_response_body_does_not_raise(self, caplog: pytest.LogCaptureFixture) -> None:
        client, _ = connected_client()

        with caplog.at_level(logging.WARNING, logger="emergent"):
            client._handle_frame(MessageType.RESPONSE, {"success": True})

        assert "malformed response" in caplog.text
