"""
Base client for socket connection management.

This module provides the BaseClient class that handles Unix socket
connections to the Emergent engine.
"""

from __future__ import annotations

import asyncio
import contextlib
import os
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ._protocol import (
    HEADER_SIZE,
    Format,
    MessageType,
    encode_frame,
    generate_correlation_id,
    try_decode_frame,
)
from .errors import (
    ConnectionError,
    DisposedError,
    ProtocolError,
    SocketNotFoundError,
    TimeoutError,
)
from .stream import MessageStream
from .types import (
    DiscoveryInfo,
    EmergentMessage,
    IpcDiscoverResponse,
    IpcEnvelope,
    IpcPushNotification,
    IpcResponse,
    IpcSubscribeRequest,
    PrimitiveInfo,
    PrimitiveKind,
    WireMessage,
)

# Default timeout for requests in seconds
DEFAULT_TIMEOUT = 30.0


def get_socket_path() -> str:
    """
    Get the socket path from environment variable.

    The Emergent engine sets `EMERGENT_SOCKET` for managed processes.

    Returns:
        The socket path string

    Raises:
        ConnectionError: If EMERGENT_SOCKET is not set
    """
    socket_path = os.environ.get("EMERGENT_SOCKET")

    if not socket_path:
        raise ConnectionError(
            "EMERGENT_SOCKET environment variable not set. "
            "Make sure the Emergent engine is running."
        )

    return socket_path


async def socket_exists(path: str) -> bool:
    """
    Check if a socket file exists.

    Args:
        path: The socket path to check

    Returns:
        True if the socket exists, False otherwise
    """
    return Path(path).exists()


@dataclass
class PendingRequest:
    """Tracks a pending request awaiting response."""

    future: asyncio.Future[IpcResponse]
    timer: asyncio.TimerHandle | None = None


@dataclass
class BaseClient:
    """
    Base client with shared connection logic.

    This class handles:
    - Unix socket connection management
    - Read loop with frame parsing
    - Push notification handling
    - Request/response correlation

    Subclasses (EmergentSource, EmergentHandler, EmergentSink) extend this
    to provide the public API.
    """

    name: str
    primitive_kind: PrimitiveKind
    timeout: float = DEFAULT_TIMEOUT
    format: Format = Format.JSON

    # Connection state
    _reader: asyncio.StreamReader | None = field(default=None, init=False, repr=False)
    _writer: asyncio.StreamWriter | None = field(default=None, init=False, repr=False)
    _read_buffer: bytearray = field(default_factory=bytearray, init=False, repr=False)
    _pending_requests: dict[str, PendingRequest] = field(
        default_factory=dict, init=False, repr=False
    )
    _message_stream: MessageStream | None = field(default=None, init=False, repr=False)
    _subscribed_types: set[str] = field(default_factory=set, init=False, repr=False)
    _read_task: asyncio.Task[None] | None = field(default=None, init=False, repr=False)
    _disposed: bool = field(default=False, init=False, repr=False)

    def subscribed_types(self) -> list[str]:
        """Get the list of currently subscribed message types."""
        return list(self._subscribed_types)

    @property
    def is_disposed(self) -> bool:
        """Check if the client has been disposed."""
        return self._disposed

    async def _connect(self, socket_path: str | None = None) -> None:
        """
        Connect to the socket.

        Args:
            socket_path: Optional custom socket path

        Raises:
            DisposedError: If the client has been disposed
            SocketNotFoundError: If the socket file doesn't exist
            ConnectionError: If connection fails
        """
        if self._disposed:
            raise DisposedError(self.__class__.__name__)

        if self._writer is not None:
            return  # Already connected

        path = socket_path if socket_path is not None else get_socket_path()

        # Check if socket exists
        if not await socket_exists(path):
            raise SocketNotFoundError(path)

        try:
            self._reader, self._writer = await asyncio.open_unix_connection(path)
            self._read_task = asyncio.create_task(self._read_loop())
        except OSError as err:
            raise ConnectionError(f"Failed to connect to {path}: {err}") from err

    async def _subscribe(self, message_types: list[str]) -> MessageStream:
        """
        Subscribe to message types.

        The SDK automatically subscribes to `system.shutdown` and handles
        graceful shutdown internally.

        Args:
            message_types: List of message types to subscribe to

        Returns:
            A MessageStream for receiving messages

        Raises:
            ConnectionError: If subscription fails
        """
        self._ensure_connected()

        correlation_id = generate_correlation_id("sub")

        # Create stream and register close callback
        stream = MessageStream(on_close=self._on_stream_close)
        self._message_stream = stream

        # Add system.shutdown to subscriptions (SDK handles it internally)
        all_types = list(message_types)
        if "system.shutdown" not in all_types:
            all_types.append("system.shutdown")

        response = await self._send_request(
            MessageType.SUBSCRIBE,
            IpcSubscribeRequest(
                correlation_id=correlation_id,
                message_types=all_types,
            ).model_dump(),
            correlation_id,
        )

        if not response.success:
            stream.close()
            raise ConnectionError(response.error or "Subscription failed")

        # Track subscribed types (exclude internal system.shutdown)
        for t in message_types:
            if t != "system.shutdown":
                self._subscribed_types.add(t)

        return stream

    async def _unsubscribe(self, message_types: list[str]) -> None:
        """
        Unsubscribe from message types.

        Args:
            message_types: List of message types to unsubscribe from
        """
        self._ensure_connected()

        correlation_id = generate_correlation_id("unsub")

        response = await self._send_request(
            MessageType.UNSUBSCRIBE,
            {
                "correlation_id": correlation_id,
                "message_types": message_types,
            },
            correlation_id,
        )

        if not response.success:
            # Log but don't fail - unsubscribe is best-effort
            pass

        # Remove from tracked types
        for t in message_types:
            self._subscribed_types.discard(t)

    async def _publish(self, message: EmergentMessage) -> None:
        """
        Publish a message.

        Args:
            message: The message to publish
        """
        self._ensure_connected()

        # Convert to wire format and set source
        wire_dict = message.to_wire().model_dump(exclude_none=True)
        wire_dict["source"] = self.name  # Always use client name as source

        # Wrap in IPC envelope (fire-and-forget, no reply expected)
        envelope = IpcEnvelope(
            correlation_id=generate_correlation_id("pub"),
            target="broker",
            message_type="Publish",
            payload=wire_dict,
            expects_reply=False,
        )

        frame = encode_frame(MessageType.REQUEST, envelope.model_dump(), self.format)
        self._writer.write(frame)  # type: ignore[union-attr]
        await self._writer.drain()  # type: ignore[union-attr]

    async def _discover(self) -> DiscoveryInfo:
        """
        Discover available message types and primitives.

        Returns:
            Discovery information from the engine

        Raises:
            ConnectionError: If discovery fails
        """
        self._ensure_connected()

        correlation_id = generate_correlation_id("disc")

        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target="broker",
            message_type="Discover",
            payload=None,
            expects_reply=True,
        )

        response = await self._send_request(
            MessageType.DISCOVER,
            envelope.model_dump(),
            correlation_id,
        )

        if not response.success:
            raise ConnectionError(response.error or "Discovery failed")

        disc_resp = IpcDiscoverResponse.model_validate(response.payload)

        return DiscoveryInfo(
            message_types=tuple(disc_resp.message_types),
            primitives=tuple(
                PrimitiveInfo(
                    name=p["name"],
                    kind=p["kind"],
                )
                for p in disc_resp.primitives
            ),
        )

    def close(self) -> None:
        """
        Close the connection.

        This is a synchronous close that cancels pending operations.
        """
        if self._disposed:
            return

        # Cancel read task
        if self._read_task is not None:
            self._read_task.cancel()
            self._read_task = None

        # Close message stream
        if self._message_stream is not None:
            self._message_stream.close()
            self._message_stream = None

        # Close connection
        if self._writer is not None:
            with contextlib.suppress(Exception):
                self._writer.close()
            self._writer = None
            self._reader = None

        # Cancel pending requests
        for pending in self._pending_requests.values():
            if pending.timer is not None:
                pending.timer.cancel()
            if not pending.future.done():
                pending.future.set_exception(ConnectionError("Connection closed"))
        self._pending_requests.clear()
        self._subscribed_types.clear()

        self._disposed = True

    async def disconnect(self) -> None:
        """
        Async close with graceful cleanup.

        This waits for the writer to close properly.
        """
        self.close()
        # Writer is already closed in close()

    def _ensure_connected(self) -> None:
        """Ensure the client is connected."""
        if self._disposed:
            raise DisposedError(self.__class__.__name__)
        if self._writer is None:
            raise ConnectionError("Not connected")

    async def _send_request(
        self,
        msg_type: MessageType,
        payload: dict[str, Any],
        correlation_id: str,
    ) -> IpcResponse:
        """
        Send a request and wait for response with timeout.

        Args:
            msg_type: The message type
            payload: The payload to send
            correlation_id: The correlation ID for response matching

        Returns:
            The response from the server

        Raises:
            TimeoutError: If the request times out
            ConnectionError: If sending fails
        """
        loop = asyncio.get_event_loop()
        future: asyncio.Future[IpcResponse] = loop.create_future()

        def timeout_callback() -> None:
            self._pending_requests.pop(correlation_id, None)
            if not future.done():
                future.set_exception(TimeoutError("Request timed out", self.timeout))

        timer = loop.call_later(self.timeout, timeout_callback)
        self._pending_requests[correlation_id] = PendingRequest(
            future=future, timer=timer
        )

        try:
            frame = encode_frame(msg_type, payload, self.format)
            self._writer.write(frame)  # type: ignore[union-attr]
            await self._writer.drain()  # type: ignore[union-attr]
            return await future
        except Exception:
            self._pending_requests.pop(correlation_id, None)
            timer.cancel()
            raise

    async def _read_loop(self) -> None:
        """Background task to receive and dispatch frames."""
        try:
            while self._reader is not None and not self._disposed:
                data = await self._reader.read(65536)
                if not data:
                    break  # EOF

                self._read_buffer.extend(data)
                self._process_frames()
        except asyncio.CancelledError:
            pass
        except Exception:
            if self._message_stream is not None:
                self._message_stream.close_with_error()

    def _process_frames(self) -> None:
        """Process complete frames from read buffer."""
        while len(self._read_buffer) >= HEADER_SIZE:
            try:
                result = try_decode_frame(self._read_buffer)
                if result is None:
                    break  # Not enough data

                # Consume the bytes
                del self._read_buffer[: result.bytes_consumed]

                # Handle the frame
                self._handle_frame(result.msg_type, result.payload)
            except ProtocolError:
                # Reset buffer on protocol error
                self._read_buffer.clear()
                break

    def _handle_frame(self, msg_type: MessageType, payload: Any) -> None:
        """Handle a received frame."""
        if msg_type == MessageType.RESPONSE:
            response = IpcResponse.model_validate(payload)
            pending = self._pending_requests.pop(response.correlation_id, None)
            if pending is not None:
                if pending.timer is not None:
                    pending.timer.cancel()
                if not pending.future.done():
                    pending.future.set_result(response)

        elif msg_type == MessageType.PUSH:
            notification = IpcPushNotification.model_validate(payload)

            # Check for shutdown signal - SDK handles this internally
            if notification.message_type == "system.shutdown":
                shutdown_payload = notification.payload
                if isinstance(shutdown_payload, dict):
                    shutdown_kind = shutdown_payload.get("kind", "").lower()

                    # Close stream if shutdown is for this primitive's kind
                    if (
                        shutdown_kind == self.primitive_kind.lower()
                        and self._message_stream is not None
                    ):
                        self._message_stream.close()
                        self._message_stream = None
                # Don't forward system.shutdown to user - it's internal
                return

            if self._message_stream is not None:
                # The notification.payload IS the serialized EmergentMessage
                wire_message = WireMessage.model_validate(notification.payload)
                message = EmergentMessage.from_wire(wire_message)
                self._message_stream.push(message)

    def _on_stream_close(self) -> None:
        """Callback when stream closes."""
        if self._message_stream is not None:
            self._message_stream = None
