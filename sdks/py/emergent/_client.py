"""
Base client for socket connection management.

This module provides the BaseClient class that handles Unix socket
connections to the Emergent engine.
"""

from __future__ import annotations

import asyncio
import contextlib
import logging
import os
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from ._protocol import (
    HEADER_SIZE,
    Format,
    MessageType,
    encode_frame,
    generate_correlation_id,
    generate_message_id,
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
    TopologyPrimitive,
    TopologyState,
    WireMessage,
)

logger = logging.getLogger("emergent")


def _init_logging(name: str = "emergent") -> None:
    """Configure logging for the emergent client.

    Logs to ``~/.local/share/emergent/<name>/primitive.log`` by default so
    the console stays clean for child-process output.  Set
    ``EMERGENT_LOG=stderr`` to log to stderr instead (for debugging).

    Reads ``EMERGENT_LOG`` or ``RUST_LOG`` env var for the level
    (default: INFO).  No-op if the application already attached a handler.
    """
    if logger.handlers:
        return

    env_val = os.environ.get("EMERGENT_LOG", os.environ.get("RUST_LOG", "INFO")).upper()
    wants_stderr = env_val == "STDERR"
    level_name = "INFO" if wants_stderr else env_val
    level = getattr(logging, level_name, logging.INFO)

    formatter = logging.Formatter("%(asctime)s %(levelname)s %(name)s: %(message)s")

    if wants_stderr:
        handler: logging.Handler = logging.StreamHandler()
    else:
        log_dir = os.path.join(
            os.environ.get("XDG_DATA_HOME", os.path.expanduser("~/.local/share")),
            "emergent",
            name,
        )
        os.makedirs(log_dir, exist_ok=True)
        handler = logging.FileHandler(os.path.join(log_dir, "primitive.log"))

    handler.setFormatter(formatter)
    logger.addHandler(handler)
    logger.setLevel(level)

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
class PendingTopologyRequest:
    """Tracks a pending topology request awaiting response."""

    future: asyncio.Future[TopologyState]
    timer: asyncio.TimerHandle | None = None


@dataclass
class PendingSubscriptionsRequest:
    """Tracks a pending subscriptions request awaiting response."""

    future: asyncio.Future[list[str]]
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

    # Auto-unwrap stdout payloads (read once from EMERGENT_UNWRAP_STDOUT)
    _unwrap_stdout: bool = field(default=False, init=False, repr=False)

    # Connection state
    _reader: asyncio.StreamReader | None = field(default=None, init=False, repr=False)
    _writer: asyncio.StreamWriter | None = field(default=None, init=False, repr=False)
    _read_buffer: bytearray = field(default_factory=bytearray, init=False, repr=False)
    _pending_requests: dict[str, PendingRequest] = field(
        default_factory=dict, init=False, repr=False
    )
    _pending_topology_requests: dict[str, PendingTopologyRequest] = field(
        default_factory=dict, init=False, repr=False
    )
    _pending_subscriptions_requests: dict[str, PendingSubscriptionsRequest] = field(
        default_factory=dict, init=False, repr=False
    )
    _message_stream: MessageStream | None = field(default=None, init=False, repr=False)
    _subscribed_types: set[str] = field(default_factory=set, init=False, repr=False)
    _read_task: asyncio.Task[None] | None = field(default=None, init=False, repr=False)
    _disposed: bool = field(default=False, init=False, repr=False)

    def __post_init__(self) -> None:
        """Read env-based configuration once at construction time."""
        env_val = os.environ.get("EMERGENT_UNWRAP_STDOUT", "")
        object.__setattr__(self, "_unwrap_stdout", env_val != "")

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
        _init_logging(self.name)

        if self._disposed:
            raise DisposedError(self.__class__.__name__)

        if self._writer is not None:
            return  # Already connected

        path = socket_path if socket_path is not None else get_socket_path()

        logger.info(
            "connecting to engine primitive=%s kind=%s path=%s",
            self.name,
            self.primitive_kind,
            path,
        )

        # Check if socket exists
        if not await socket_exists(path):
            logger.error("engine socket not found path=%s", path)
            raise SocketNotFoundError(path)

        try:
            self._reader, self._writer = await asyncio.open_unix_connection(path)
            self._read_task = asyncio.create_task(self._read_loop())
            logger.info(
                "connected to engine primitive=%s kind=%s",
                self.name,
                self.primitive_kind,
            )
        except OSError as err:
            logger.error(
                "failed to connect to engine primitive=%s error=%s", self.name, err
            )
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

        logger.info(
            "subscribing to message types primitive=%s types=%s",
            self.name,
            message_types,
        )

        response = await self._send_request(
            MessageType.SUBSCRIBE,
            IpcSubscribeRequest(
                correlation_id=correlation_id,
                message_types=all_types,
            ).model_dump(),
            correlation_id,
        )

        if not response.success:
            logger.error(
                "subscription failed primitive=%s error=%s",
                self.name,
                response.error,
            )
            stream.close()
            raise ConnectionError(response.error or "Subscription failed")

        # Track subscribed types (exclude internal system.shutdown)
        for t in message_types:
            if t != "system.shutdown":
                self._subscribed_types.add(t)

        logger.info("subscribed to message types primitive=%s", self.name)

        return stream

    async def _unsubscribe(self, message_types: list[str]) -> None:
        """
        Unsubscribe from message types.

        Args:
            message_types: List of message types to unsubscribe from
        """
        self._ensure_connected()

        logger.debug(
            "unsubscribing from message types primitive=%s types=%s",
            self.name,
            message_types,
        )

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
            logger.warning(
                "unsubscribe failed primitive=%s types=%s error=%s",
                self.name,
                message_types,
                response.error,
            )

        # Remove from tracked types
        for t in message_types:
            self._subscribed_types.discard(t)

        logger.debug(
            "unsubscribed from message types primitive=%s", self.name
        )

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

        # Wrap in IpcEmergentMessage format (matches Rust: IpcEmergentMessage { inner: message })
        ipc_message = {"inner": wire_dict}

        # Wrap in IPC envelope (fire-and-forget, no reply expected)
        # Target "message_broker" and type "EmergentMessage" match engine expectations
        envelope = IpcEnvelope(
            correlation_id=generate_correlation_id("pub"),
            target="message_broker",
            message_type="EmergentMessage",
            payload=ipc_message,
            expects_reply=False,
        )

        frame = encode_frame(MessageType.REQUEST, envelope.model_dump(), self.format)

        logger.debug(
            "publishing message primitive=%s message_type=%s message_id=%s",
            self.name,
            wire_dict.get("message_type"),
            wire_dict.get("id"),
        )

        try:
            self._writer.write(frame)  # type: ignore[union-attr]
            await self._writer.drain()  # type: ignore[union-attr]
        except Exception as e:
            logger.error(
                "failed to publish message primitive=%s error=%s", self.name, e
            )
            raise

        logger.debug(
            "published message primitive=%s message_type=%s message_id=%s",
            self.name,
            wire_dict.get("message_type"),
            wire_dict.get("id"),
        )

    async def _publish_ack(self, message: EmergentMessage) -> None:
        """
        Publish a message with broker acknowledgment (backpressure).

        Unlike ``_publish``, this waits for the engine's message broker to
        confirm it has processed and forwarded the message before returning.

        Args:
            message: The message to publish
        """
        self._ensure_connected()

        wire_dict = message.to_wire().model_dump(exclude_none=True)
        wire_dict["source"] = self.name

        ipc_message = {"inner": wire_dict}

        correlation_id = generate_correlation_id("pub")
        envelope = IpcEnvelope(
            correlation_id=correlation_id,
            target="message_broker",
            message_type="EmergentMessage",
            payload=ipc_message,
            expects_reply=True,
        )

        logger.debug(
            "publishing message (ack) primitive=%s message_type=%s",
            self.name,
            wire_dict.get("message_type"),
        )

        response = await self._send_request(
            MessageType.REQUEST,
            envelope.model_dump(),
            correlation_id,
        )

        if not response.success:
            from .errors import PublishError

            raise PublishError(
                response.error or "Broker returned error",
                message_type=wire_dict.get("message_type", ""),
            )

        logger.debug(
            "publish_ack succeeded primitive=%s message_type=%s",
            self.name,
            wire_dict.get("message_type"),
        )

    async def _discover(self) -> DiscoveryInfo:
        """
        Discover available message types and primitives.

        Returns:
            Discovery information from the engine

        Raises:
            ConnectionError: If discovery fails
        """
        self._ensure_connected()

        logger.debug("sending discovery request primitive=%s", self.name)

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
            logger.error(
                "discovery failed primitive=%s error=%s", self.name, response.error
            )
            raise ConnectionError(response.error or "Discovery failed")

        disc_resp = IpcDiscoverResponse.model_validate(response.payload)

        logger.debug(
            "discovery complete primitive=%s message_types=%d primitives=%d",
            self.name,
            len(disc_resp.message_types),
            len(disc_resp.primitives),
        )

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

    async def _get_my_subscriptions(self) -> list[str]:
        """
        Get the configured subscription types for this primitive.

        Uses pub/sub pattern: publishes `system.request.subscriptions` and
        waits for `system.response.subscriptions` with matching correlation_id.

        Returns:
            List of message type names to subscribe to

        Raises:
            ConnectionError: If the request fails
            TimeoutError: If the request times out
        """
        self._ensure_connected()

        logger.debug(
            "querying configured subscriptions primitive=%s", self.name
        )

        correlation_id = generate_correlation_id("cor")

        # Subscribe to response type first
        sub_correlation_id = generate_correlation_id("sub")
        sub_response = await self._send_request(
            MessageType.SUBSCRIBE,
            IpcSubscribeRequest(
                correlation_id=sub_correlation_id,
                message_types=["system.response.subscriptions"],
            ).model_dump(),
            sub_correlation_id,
        )

        if not sub_response.success:
            raise ConnectionError(
                sub_response.error or "Failed to subscribe to response type"
            )

        # Create promise to wait for response
        loop = asyncio.get_event_loop()
        future: asyncio.Future[list[str]] = loop.create_future()

        def timeout_callback() -> None:
            self._pending_subscriptions_requests.pop(correlation_id, None)
            if not future.done():
                future.set_exception(
                    TimeoutError("GetSubscriptions request timed out", self.timeout)
                )

        timer = loop.call_later(self.timeout, timeout_callback)
        self._pending_subscriptions_requests[correlation_id] = (
            PendingSubscriptionsRequest(future=future, timer=timer)
        )

        # Create and publish request message
        request = EmergentMessage(
            id=generate_message_id(),
            message_type="system.request.subscriptions",
            source=self.name,
            correlation_id=correlation_id,
            timestamp_ms=int(time.time() * 1000),
            payload={"name": self.name},
        )
        await self._publish(request)

        # Wait for response
        result = await future
        logger.info(
            "received configured subscriptions primitive=%s types=%s",
            self.name,
            result,
        )
        return result

    async def _get_topology(self) -> TopologyState:
        """
        Get the current topology (all primitives and their state).

        Uses pub/sub pattern: publishes `system.request.topology` and
        waits for `system.response.topology` with matching correlation_id.

        Returns:
            TopologyState with all primitives

        Raises:
            ConnectionError: If the request fails
            TimeoutError: If the request times out
        """
        self._ensure_connected()

        logger.debug("querying topology primitive=%s", self.name)

        correlation_id = generate_correlation_id("cor")

        # Subscribe to response type first
        sub_correlation_id = generate_correlation_id("sub")
        sub_response = await self._send_request(
            MessageType.SUBSCRIBE,
            IpcSubscribeRequest(
                correlation_id=sub_correlation_id,
                message_types=["system.response.topology"],
            ).model_dump(),
            sub_correlation_id,
        )

        if not sub_response.success:
            raise ConnectionError(
                sub_response.error or "Failed to subscribe to response type"
            )

        # Create promise to wait for response
        loop = asyncio.get_event_loop()
        future: asyncio.Future[TopologyState] = loop.create_future()

        def timeout_callback() -> None:
            self._pending_topology_requests.pop(correlation_id, None)
            if not future.done():
                future.set_exception(
                    TimeoutError("GetTopology request timed out", self.timeout)
                )

        timer = loop.call_later(self.timeout, timeout_callback)
        self._pending_topology_requests[correlation_id] = PendingTopologyRequest(
            future=future, timer=timer
        )

        # Create and publish request message
        request = EmergentMessage(
            id=generate_message_id(),
            message_type="system.request.topology",
            source=self.name,
            correlation_id=correlation_id,
            timestamp_ms=int(time.time() * 1000),
            payload={},
        )
        await self._publish(request)

        # Wait for response
        result = await future
        logger.debug(
            "received topology primitive=%s primitive_count=%d",
            self.name,
            len(result.primitives),
        )
        return result

    def close(self) -> None:
        """
        Close the connection.

        This is a synchronous close that cancels pending operations.
        """
        logger.info(
            "disconnecting from engine primitive=%s kind=%s",
            self.name,
            self.primitive_kind,
        )

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

        # Cancel pending topology requests
        for pending in self._pending_topology_requests.values():
            if pending.timer is not None:
                pending.timer.cancel()
            if not pending.future.done():
                pending.future.set_exception(ConnectionError("Connection closed"))
        self._pending_topology_requests.clear()

        # Cancel pending subscriptions requests
        for pending in self._pending_subscriptions_requests.values():
            if pending.timer is not None:
                pending.timer.cancel()
            if not pending.future.done():
                pending.future.set_exception(ConnectionError("Connection closed"))
        self._pending_subscriptions_requests.clear()

        self._subscribed_types.clear()

        logger.info("disconnected from engine primitive=%s", self.name)

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
        logger.debug("read loop started primitive=%s", self.name)
        try:
            while self._reader is not None and not self._disposed:
                data = await self._reader.read(65536)
                if not data:
                    logger.info(
                        "connection closed (EOF) primitive=%s", self.name
                    )
                    break  # EOF

                self._read_buffer.extend(data)
                self._process_frames()
        except asyncio.CancelledError:
            pass
        except Exception as e:
            logger.error(
                "read loop error primitive=%s error=%s", self.name, e
            )
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
            except ProtocolError as e:
                logger.error(
                    "protocol error while processing frame primitive=%s error=%s",
                    self.name,
                    e,
                )
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

                    logger.info(
                        "received shutdown signal primitive=%s kind=%s",
                        self.name,
                        shutdown_kind,
                    )

                    # Close stream if shutdown is for this primitive's kind
                    if (
                        shutdown_kind == self.primitive_kind.lower()
                        and self._message_stream is not None
                    ):
                        logger.info(
                            "shutting down (engine requested) primitive=%s",
                            self.name,
                        )
                        self._message_stream.close()
                        self._message_stream = None
                # Don't forward system.shutdown to user - it's internal
                return

            # Handle system.response.topology messages
            if notification.message_type == "system.response.topology":
                wire_message = WireMessage.model_validate(notification.payload)
                correlation_id = wire_message.correlation_id
                if correlation_id:
                    pending = self._pending_topology_requests.pop(correlation_id, None)
                    if pending is not None:
                        if pending.timer is not None:
                            pending.timer.cancel()
                        if not pending.future.done():
                            # Extract primitives from payload
                            payload_data = wire_message.payload or {}
                            primitives_data = (
                                payload_data.get("primitives", [])
                                if isinstance(payload_data, dict)
                                else []
                            )
                            primitives = tuple(
                                TopologyPrimitive(
                                    name=p.get("name", ""),
                                    kind=p.get("kind", ""),
                                    state=p.get("state", "stopped"),
                                    publishes=tuple(p.get("publishes", [])),
                                    subscribes=tuple(p.get("subscribes", [])),
                                    pid=p.get("pid"),
                                    error=p.get("error"),
                                )
                                for p in primitives_data
                            )
                            pending.future.set_result(
                                TopologyState(primitives=primitives)
                            )
                return  # Don't forward to message stream

            # Handle system.response.subscriptions messages
            if notification.message_type == "system.response.subscriptions":
                wire_message = WireMessage.model_validate(notification.payload)
                correlation_id = wire_message.correlation_id
                if correlation_id:
                    pending = self._pending_subscriptions_requests.pop(
                        correlation_id, None
                    )
                    if pending is not None:
                        if pending.timer is not None:
                            pending.timer.cancel()
                        if not pending.future.done():
                            # Extract subscribes from payload
                            payload_data = wire_message.payload or {}
                            subscribes = (
                                payload_data.get("subscribes", [])
                                if isinstance(payload_data, dict)
                                else []
                            )
                            pending.future.set_result(subscribes)
                return  # Don't forward to message stream

            if self._message_stream is not None:
                logger.debug(
                    "received message primitive=%s message_type=%s",
                    self.name,
                    notification.message_type,
                )
                # The notification.payload IS the serialized EmergentMessage
                wire_message = WireMessage.model_validate(notification.payload)
                message = EmergentMessage.from_wire(wire_message)

                # Auto-unwrap exec-source stdout payloads when enabled
                if (
                    self._unwrap_stdout
                    and not message.message_type.startswith("system.")
                ):
                    message = message.unwrap_stdout()

                self._message_stream.push(message)

    def _on_stream_close(self) -> None:
        """Callback when stream closes."""
        if self._message_stream is not None:
            self._message_stream = None
