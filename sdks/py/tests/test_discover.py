"""Tests for discover().

The discovery response below was captured from an engine on acton-reactive
9.3.0. Its lists sit at the top level of the frame body, beside
``correlation_id`` and ``success``, and not under ``payload``.
"""

import asyncio
from typing import Any

import pytest

from emergent._client import discovery_info_from_response, response_from_frame
from emergent._protocol import MessageType
from emergent.errors import ConnectionError
from emergent.sink import EmergentSink
from emergent.types import DiscoveryInfo, IpcResponse, PrimitiveInfo

from .conftest import _TestEngine
from .test_error_frames import connected_client, wait_for_frame

ENGINE_DISCOVERY_RESPONSE: dict[str, Any] = {
    "correlation_id": "disc_new",
    "success": True,
    "protocol_version": {
        "current": 2,
        "min_supported": 1,
        "max_supported": 2,
        "description": "v2 (multi-format, streaming, push, discovery)",
        "capabilities": {
            "messagepack": True,
            "streaming": True,
            "push": True,
            "discovery": True,
        },
    },
    "actors": [
        {
            "name": "message_broker",
            "ern": "ern:acton:reactive:component:message_broker_01m2xw6x0jfhsrmbxrg90t657d",
        }
    ],
    "message_types": ["SystemEvent", "EmergentMessage"],
}


def engine_response(**changes: Any) -> IpcResponse:
    body = {**ENGINE_DISCOVERY_RESPONSE, **changes}
    response = response_from_frame(MessageType.RESPONSE, body)
    assert response is not None
    return response


class TestDiscoveryInfoFromResponse:
    """The pure mapping from the engine's response to DiscoveryInfo."""

    def test_reads_the_engine_response(self) -> None:
        info = discovery_info_from_response(engine_response())

        assert info == DiscoveryInfo(
            message_types=("SystemEvent", "EmergentMessage"),
            primitives=(PrimitiveInfo(name="message_broker"),),
        )

    def test_the_engine_reports_no_kind(self) -> None:
        info = discovery_info_from_response(engine_response())

        assert [p.kind for p in info.primitives] == [None]

    def test_a_list_the_engine_left_out_reads_as_empty(self) -> None:
        body = {"correlation_id": "disc_1", "success": True}
        response = response_from_frame(MessageType.RESPONSE, body)
        assert response is not None

        info = discovery_info_from_response(response)

        assert info.message_types == ()
        assert info.primitives == ()


class TestDiscoverRequest:
    """What discover() writes, and how it reads each kind of reply."""

    async def test_sends_the_discover_opcode_and_request_body(self) -> None:
        client, writer = connected_client()

        task = asyncio.create_task(client._discover())
        await wait_for_frame(writer)

        assert writer.frames[0][5] == 0x08
        msg_type, request = writer.sent()
        assert msg_type is MessageType.DISCOVER
        assert request["correlation_id"].startswith("disc_")
        assert request["include_actors"] is True
        assert request["include_message_types"] is True

        reply = {**ENGINE_DISCOVERY_RESPONSE, "correlation_id": request["correlation_id"]}
        client._handle_frame(MessageType.RESPONSE, reply)

        info = await asyncio.wait_for(task, timeout=1.0)
        assert info.message_types == ("SystemEvent", "EmergentMessage")
        assert [p.name for p in info.primitives] == ["message_broker"]

    async def test_error_frame_fails_discover_with_the_engine_text(self) -> None:
        client, writer = connected_client()

        task = asyncio.create_task(client._discover())
        await wait_for_frame(writer)
        _, request = writer.sent()

        client._handle_frame(
            MessageType.ERROR,
            {
                "correlation_id": request["correlation_id"],
                "success": False,
                "error": "Parse error: bad discover request",
            },
        )

        with pytest.raises(ConnectionError, match="Parse error: bad discover request"):
            await asyncio.wait_for(task, timeout=1.0)

    async def test_malformed_reply_is_a_connection_error(self) -> None:
        client, writer = connected_client()

        task = asyncio.create_task(client._discover())
        await wait_for_frame(writer)
        _, request = writer.sent()

        client._handle_frame(
            MessageType.RESPONSE,
            {
                "correlation_id": request["correlation_id"],
                "success": True,
                "message_types": "not a list",
            },
        )

        with pytest.raises(ConnectionError, match="Malformed discovery response"):
            await asyncio.wait_for(task, timeout=1.0)


async def test_discover_against_a_running_engine(engine: _TestEngine) -> None:
    async with await EmergentSink.connect(
        "discover_sink", socket_path=str(engine.socket_path), timeout=5.0
    ) as sink:
        info = await sink.discover()

    assert "EmergentMessage" in info.message_types
    assert "message_broker" in [p.name for p in info.primitives]
