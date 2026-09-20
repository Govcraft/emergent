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
from typing import TYPE_CHECKING, Any, Protocol

from pydantic import ValidationError

from ._protocol import (
    DEFAULT_FORMAT,
    HEADER_SIZE,
    BadFrameBody,
    BadFraming,
    Format,
    MessageType,
    encode_frame,
    generate_correlation_id,
    generate_message_id,
    next_frame,
)
from .errors import (
    ConnectionError,
    DiscoveryError,
    DisposedError,
    EmergentError,
    PublishError,
    SocketNotFoundError,
    SubscriptionError,
    TimeoutError,
)
from .stream import MessageStream
from .topics import is_emergent_message_type, partition_topics
from .types import (
    DiscoveryInfo,
    EmergentMessage,
    IpcDiscoverRequest,
    IpcDiscoverResponse,
    IpcEnvelope,
    IpcPatternSubscribeRequest,
    IpcPushNotification,
    IpcResponse,
    IpcSubscribeRequest,
    PrimitiveInfo,
    PrimitiveKind,
    TopologyPrimitive,
    TopologyState,
    WireMessage,
)

if TYPE_CHECKING:
    from collections.abc import Callable

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


def extract_shutdown_kind(notification_payload: Any) -> str | None:
    """Read the primitive kind a ``system.shutdown`` broadcast targets.

    The engine delivers the whole message envelope as the notification
    payload, so the kind sits at ``payload.kind`` inside that envelope. A bare
    ``{"kind": ...}`` object is accepted too, which keeps a hand-written
    broadcast working. Returns ``None`` when no string kind is present. The
    result is lowercased so callers can compare it against a primitive kind
    directly.
    """
    if not isinstance(notification_payload, dict):
        return None
    inner = notification_payload.get("payload")
    candidates = (inner, notification_payload)
    for candidate in candidates:
        if isinstance(candidate, dict):
            kind = candidate.get("kind")
            if isinstance(kind, str):
                return kind.lower()
    return None


# Error text for an ERROR frame that arrives without any of its own
DEFAULT_ERROR_TEXT = "Engine returned an error"


def response_from_frame(msg_type: MessageType, payload: Any) -> IpcResponse | None:
    """Read the response a ``RESPONSE`` or ``ERROR`` frame carries.

    The engine answers a failed request with an ``ERROR`` frame whose body is
    the same response object a ``RESPONSE`` frame carries: ``correlation_id``,
    ``success``, ``error`` and ``error_code``. An ``ERROR`` frame always reads
    as a failure here, whatever its ``success`` field says, and always has
    error text. Returns ``None`` for any other frame type and for a body with
    no string ``correlation_id``, since nothing can be matched to it.
    """
    if msg_type not in (MessageType.RESPONSE, MessageType.ERROR):
        return None
    if not isinstance(payload, dict) or not isinstance(payload.get("correlation_id"), str):
        return None
    try:
        response = IpcResponse.model_validate(payload)
    except ValidationError:
        return None
    if msg_type == MessageType.ERROR:
        return response.model_copy(
            update={"success": False, "error": response.error or DEFAULT_ERROR_TEXT}
        )
    return response


def error_frame_text(payload: Any) -> str:
    """Describe an ``ERROR`` frame body for a log line.

    Gives the error text, followed by the error code in parentheses when the
    engine sent one. Falls back to ``DEFAULT_ERROR_TEXT`` for a body that holds
    no error text.
    """
    if not isinstance(payload, dict):
        return DEFAULT_ERROR_TEXT
    error = payload.get("error")
    text = error if isinstance(error, str) and error else DEFAULT_ERROR_TEXT
    code = payload.get("error_code")
    return f"{text} ({code})" if isinstance(code, str) and code else text


def push_from_frame(payload: Any) -> IpcPushNotification | None:
    """Read the notification a ``PUSH`` frame carries.

    Returns ``None`` for a body that is not a notification, such as ``None``
    or one whose ``message_type`` is not a string.
    """
    try:
        return IpcPushNotification.model_validate(payload)
    except ValidationError:
        return None


def wire_message_from_push(payload: Any) -> WireMessage | None:
    """Read the Emergent message a push notification carries as its payload.

    Returns ``None`` for anything that is not a whole message, so a message of
    the wrong shape never reaches the subscriber.
    """
    try:
        return WireMessage.model_validate(payload)
    except ValidationError:
        return None


def topology_from_payload(payload: Any) -> TopologyState:
    """Read the primitives a ``system.response.topology`` payload lists.

    A field an entry leaves out takes its default. Entries that are not
    objects, or hold a field of the wrong type, are dropped. A payload with no
    ``primitives`` list reads as an empty topology.
    """
    entries = payload.get("primitives") if isinstance(payload, dict) else None
    if not isinstance(entries, list):
        return TopologyState(primitives=())

    primitives: list[TopologyPrimitive] = []
    for entry in entries:
        if not isinstance(entry, dict):
            continue
        try:
            primitives.append(
                TopologyPrimitive.model_validate(
                    {"name": "", "kind": "", "state": "stopped"} | entry
                )
            )
        except ValidationError:
            continue
    return TopologyState(primitives=tuple(primitives))


def subscribes_from_payload(payload: Any) -> list[str]:
    """Read the topics a ``system.response.subscriptions`` payload lists.

    Entries that are not strings are dropped. A payload with no ``subscribes``
    list reads as no subscriptions.
    """
    entries = payload.get("subscribes") if isinstance(payload, dict) else None
    if not isinstance(entries, list):
        return []
    return [entry for entry in entries if isinstance(entry, str)]


def discovery_info_from_response(response: IpcResponse) -> DiscoveryInfo:
    """Build ``DiscoveryInfo`` from a successful discovery response.

    The engine writes ``actors`` and ``message_types`` at the top level of the
    response, and leaves out whichever list was not asked for. Each actor
    becomes a ``PrimitiveInfo`` with no kind, as in the Rust SDK, because the
    response does not carry one.

    Raises:
        pydantic.ValidationError: If the response is not a discovery response
    """
    body = IpcDiscoverResponse.model_validate(response.model_dump())
    return DiscoveryInfo(
        message_types=tuple(body.message_types or ()),
        primitives=tuple(PrimitiveInfo(name=actor.name) for actor in body.actors or ()),
    )


def write_failure(cause: OSError, publishing: str | None) -> EmergentError:
    """Name a socket write the operating system refused.

    ``publishing`` is the message type when the write carried a publish, and
    ``None`` for every other request. A refused publish is a ``PublishError``
    and a refused request a ``ConnectionError``. The caller raises the result
    ``from cause``, so the OS error stays reachable as ``__cause__``.
    """
    if publishing is not None:
        return PublishError(f"Failed to publish: {cause}", message_type=publishing)
    return ConnectionError(f"Failed to send: {cause}")


def parse_unwrap_flag(value: str | None) -> bool:
    """Decide whether ``EMERGENT_UNWRAP_STDOUT`` switches stdout unwrapping on.

    Surrounding whitespace and letter case are ignored, and only ``"true"``
    and ``"1"`` enable it, the same rule as the Rust, Go and TypeScript SDKs.
    Anything else, including ``"false"``, ``"0"``, and an unset variable,
    leaves it off.
    """
    return value is not None and value.strip().lower() in ("true", "1")


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


class TimedPending(Protocol):
    """A pending entry that may hold the timer for its timeout."""

    timer: asyncio.TimerHandle | None


def discard_pending[P: TimedPending](pending: dict[str, P], correlation_id: str) -> None:
    """
    Remove a pending entry and cancel its timer.

    For the caller that gives up on a request, whatever the reason. An entry
    already settled or timed out is gone, and that is not an error.
    """
    entry = pending.pop(correlation_id, None)
    if entry is not None and entry.timer is not None:
        entry.timer.cancel()


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
    format: Format = DEFAULT_FORMAT

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
    _connection_lost: bool = field(default=False, init=False, repr=False)
    _on_connection_lost: Callable[[], None] | None = field(default=None, init=False, repr=False)

    def __post_init__(self) -> None:
        """Read env-based configuration once at construction time."""
        env_val = os.environ.get("EMERGENT_UNWRAP_STDOUT")
        object.__setattr__(self, "_unwrap_stdout", parse_unwrap_flag(env_val))

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
            logger.error("failed to connect to engine primitive=%s error=%s", self.name, err)
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

        # Split before anything is sent, so a topic that could never match,
        # such as "system.*.error", raises here instead of subscribing to
        # silence.
        exact_types, patterns = partition_topics(list(message_types))

        correlation_id = generate_correlation_id("sub")

        # Create stream and register close callback
        stream = MessageStream(on_close=lambda: self._on_stream_close(stream))
        self._message_stream = stream

        # Add system.shutdown to subscriptions (SDK handles it internally)
        all_types = list(exact_types)
        if "system.shutdown" not in all_types:
            all_types.append("system.shutdown")

        logger.info(
            "subscribing to message types primitive=%s types=%s patterns=%s",
            self.name,
            exact_types,
            patterns,
        )

        response = await self._subscribe_request(
            stream,
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
            raise SubscriptionError(response.error or "Subscription failed", exact_types)

        if patterns:
            # Patterns travel on their own request because the engine keeps a
            # separate index for them. A connection matching a message through
            # both indexes still receives one copy.
            pattern_correlation_id = generate_correlation_id("psub")
            pattern_response = await self._subscribe_request(
                stream,
                MessageType.SUBSCRIBE_PATTERNS,
                IpcPatternSubscribeRequest(
                    correlation_id=pattern_correlation_id,
                    patterns=patterns,
                ).model_dump(),
                pattern_correlation_id,
            )

            if not pattern_response.success:
                logger.error(
                    "pattern subscription failed primitive=%s error=%s",
                    self.name,
                    pattern_response.error,
                )
                stream.close()
                raise SubscriptionError(
                    pattern_response.error or "Pattern subscription failed", patterns
                )

        # Track subscribed types (exclude internal system.shutdown)
        for t in message_types:
            if t != "system.shutdown":
                self._subscribed_types.add(t)

        logger.info("subscribed to message types primitive=%s", self.name)

        return stream

    async def _subscribe_request(
        self,
        stream: MessageStream,
        msg_type: MessageType,
        payload: dict[str, Any],
        correlation_id: str,
    ) -> IpcResponse:
        """
        Send one request of a subscribe.

        When the request raises, the caller never receives the stream, so it is
        closed here, which also unregisters it.
        """
        try:
            return await self._send_request(msg_type, payload, correlation_id)
        except BaseException:
            stream.close()
            raise

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

        exact_types, patterns = partition_topics(list(message_types))

        correlation_id = generate_correlation_id("unsub")

        response = await self._send_request(
            MessageType.UNSUBSCRIBE,
            {
                "correlation_id": correlation_id,
                "message_types": exact_types,
            },
            correlation_id,
        )

        if not response.success:
            # Log but don't fail - unsubscribe is best-effort
            logger.warning(
                "unsubscribe failed primitive=%s types=%s error=%s",
                self.name,
                exact_types,
                response.error,
            )

        if patterns:
            pattern_correlation_id = generate_correlation_id("punsub")
            pattern_response = await self._send_request(
                MessageType.UNSUBSCRIBE_PATTERNS,
                {
                    "correlation_id": pattern_correlation_id,
                    "patterns": patterns,
                },
                pattern_correlation_id,
            )
            if not pattern_response.success:
                logger.warning(
                    "pattern unsubscribe failed primitive=%s patterns=%s error=%s",
                    self.name,
                    patterns,
                    pattern_response.error,
                )

        # Remove from tracked types
        for t in message_types:
            self._subscribed_types.discard(t)

        logger.debug("unsubscribed from message types primitive=%s", self.name)

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
        except OSError as e:
            logger.error("failed to publish message primitive=%s error=%s", self.name, e)
            raise write_failure(e, wire_dict.get("message_type", "")) from e

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
            publishing=wire_dict.get("message_type", ""),
        )

        if not response.success:
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

        response = await self._send_request(
            MessageType.DISCOVER,
            IpcDiscoverRequest(correlation_id=correlation_id).model_dump(),
            correlation_id,
        )

        if not response.success:
            logger.error("discovery failed primitive=%s error=%s", self.name, response.error)
            raise DiscoveryError(response.error or "Discovery failed")

        try:
            info = discovery_info_from_response(response)
        except ValidationError as err:
            logger.error("discovery response malformed primitive=%s error=%s", self.name, err)
            raise DiscoveryError(f"Malformed discovery response: {err}") from err

        logger.debug(
            "discovery complete primitive=%s message_types=%d primitives=%d",
            self.name,
            len(info.message_types),
            len(info.primitives),
        )

        return info

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

        logger.debug("querying configured subscriptions primitive=%s", self.name)

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
            raise SubscriptionError(
                sub_response.error or "Failed to subscribe to response type",
                ["system.response.subscriptions"],
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
        self._pending_subscriptions_requests[correlation_id] = PendingSubscriptionsRequest(
            future=future, timer=timer
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
        try:
            await self._publish(request)
            result = await future
        finally:
            # Settled, timed out, refused or cancelled: nothing stays behind.
            discard_pending(self._pending_subscriptions_requests, correlation_id)
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
            raise SubscriptionError(
                sub_response.error or "Failed to subscribe to response type",
                ["system.response.topology"],
            )

        # Create promise to wait for response
        loop = asyncio.get_event_loop()
        future: asyncio.Future[TopologyState] = loop.create_future()

        def timeout_callback() -> None:
            self._pending_topology_requests.pop(correlation_id, None)
            if not future.done():
                future.set_exception(TimeoutError("GetTopology request timed out", self.timeout))

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
        try:
            await self._publish(request)
            result = await future
        finally:
            # Settled, timed out, refused or cancelled: nothing stays behind.
            discard_pending(self._pending_topology_requests, correlation_id)
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

        # Close connection
        if self._writer is not None:
            with contextlib.suppress(Exception):
                self._writer.close()
            self._writer = None
            self._reader = None

        self._fail_everything_pending()

        self._subscribed_types.clear()

        logger.info("disconnected from engine primitive=%s", self.name)

        self._disposed = True

    def _fail_everything_pending(self) -> None:
        """
        End the message stream and fail every request still in flight.

        For a connection that is gone, whether the caller closed it or the
        engine did. Nothing pending can be answered any more, so each request
        and query fails with a ConnectionError now and does not wait out its
        timer, and an ``async for`` over the stream stops.
        """
        if self._message_stream is not None:
            self._message_stream.close()
            self._message_stream = None

        in_flight: list[dict[str, Any]] = [
            self._pending_requests,
            self._pending_topology_requests,
            self._pending_subscriptions_requests,
        ]
        for pending_map in in_flight:
            for pending in pending_map.values():
                if pending.timer is not None:
                    pending.timer.cancel()
                if not pending.future.done():
                    pending.future.set_exception(ConnectionError("Connection closed"))
            pending_map.clear()

    def _engine_closed_the_connection(self) -> None:
        """
        Settle a connection that ended without ``close()`` being called.

        The engine is gone, so nothing in flight can be answered. Whoever asked
        to be told is told after that, once.
        """
        self._fail_everything_pending()
        self._connection_lost = True
        if self._on_connection_lost is not None:
            self._on_connection_lost()

    def _when_connection_lost(self, callback: Callable[[], None]) -> None:
        """
        Call ``callback`` when the engine closes the connection.

        A connection that is already lost calls it at once. ``close()`` never
        calls it. There is one callback, and a second call replaces the first.
        """
        self._on_connection_lost = callback
        if self._connection_lost:
            callback()

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
        publishing: str | None = None,
    ) -> IpcResponse:
        """
        Send a request and wait for response with timeout.

        Args:
            msg_type: The message type
            payload: The payload to send
            correlation_id: The correlation ID for response matching
            publishing: The message type when the request carries a publish

        Returns:
            The response from the server

        Raises:
            TimeoutError: If the request times out
            ConnectionError: If the socket refuses the write
            PublishError: If the socket refuses the write and ``publishing`` is set
        """
        loop = asyncio.get_event_loop()
        future: asyncio.Future[IpcResponse] = loop.create_future()

        def timeout_callback() -> None:
            self._pending_requests.pop(correlation_id, None)
            if not future.done():
                future.set_exception(TimeoutError("Request timed out", self.timeout))

        timer = loop.call_later(self.timeout, timeout_callback)
        self._pending_requests[correlation_id] = PendingRequest(future=future, timer=timer)

        try:
            frame = encode_frame(msg_type, payload, self.format)
            try:
                self._writer.write(frame)  # type: ignore[union-attr]
                await self._writer.drain()  # type: ignore[union-attr]
            except OSError as e:
                logger.error("failed to send request primitive=%s error=%s", self.name, e)
                raise write_failure(e, publishing) from e
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
                    logger.info("connection closed (EOF) primitive=%s", self.name)
                    break  # EOF

                self._read_buffer.extend(data)
                self._process_frames()
        except asyncio.CancelledError:
            # close() cancelled the loop and settles everything itself.
            return
        except Exception as e:
            logger.error("read loop error primitive=%s error=%s", self.name, e)

        self._engine_closed_the_connection()

    def _process_frames(self) -> None:
        """Process complete frames from read buffer."""
        while len(self._read_buffer) >= HEADER_SIZE:
            step = next_frame(self._read_buffer)
            if step is None:
                break  # Not enough data

            if isinstance(step, BadFraming):
                # Nothing says where the next frame starts, so drop what is
                # buffered.
                logger.error(
                    "protocol error while processing frame primitive=%s error=%s",
                    self.name,
                    step.reason,
                )
                self._read_buffer.clear()
                break

            # Consume the bytes
            del self._read_buffer[: step.bytes_consumed]

            if isinstance(step, BadFrameBody):
                logger.warning(
                    "skipping frame with malformed body primitive=%s msg_type=%#04x error=%s",
                    self.name,
                    step.raw_msg_type,
                    step.reason,
                )
                continue

            if step.msg_type is None:
                # The length prefix already delimited the frame, so
                # skipping it keeps the stream in step.
                logger.warning(
                    "skipping frame of unknown type primitive=%s msg_type=%#04x",
                    self.name,
                    step.raw_msg_type,
                )
                continue

            # One frame must never end the read loop, whatever handling it
            # raises.
            try:
                self._handle_frame(step.msg_type, step.payload)
            except Exception as e:
                logger.error(
                    "skipping frame that could not be handled primitive=%s msg_type=%s error=%s",
                    self.name,
                    step.msg_type.name,
                    e,
                )

    def _settle_response(self, msg_type: MessageType, payload: Any) -> None:
        """
        Settle the pending request a RESPONSE or ERROR frame answers.

        A failed request comes back as an ERROR frame. It settles the pending
        request like any response, with ``success`` False, and the caller
        raises its own error from the engine's error text.
        """
        response = response_from_frame(msg_type, payload)
        pending = (
            self._pending_requests.pop(response.correlation_id, None)
            if response is not None
            else None
        )

        if pending is not None and response is not None:
            if pending.timer is not None:
                pending.timer.cancel()
            if not pending.future.done():
                pending.future.set_result(response)
        elif msg_type == MessageType.ERROR:
            # No request to fail, for example a connection-level rejection or
            # a request the engine could not parse.
            logger.error(
                "engine error matched no pending request primitive=%s error=%s",
                self.name,
                error_frame_text(payload),
            )
        elif response is None:
            logger.warning("dropping malformed response frame primitive=%s", self.name)

    def _handle_frame(self, msg_type: MessageType, payload: Any) -> None:
        """Handle a received frame."""
        if msg_type in (MessageType.RESPONSE, MessageType.ERROR):
            self._settle_response(msg_type, payload)

        elif msg_type in (MessageType.HEARTBEAT, MessageType.STREAM):
            # The engine echoes a heartbeat only after receiving one, and
            # streams only to a request that asked for a stream. This SDK
            # sends neither, so there is nothing to route.
            logger.debug("ignoring frame primitive=%s msg_type=%s", self.name, msg_type.name)

        elif msg_type == MessageType.PUSH:
            self._handle_push(payload)

    def _handle_push(self, payload: Any) -> None:
        """
        Route a PUSH frame.

        The system messages the SDK owns are handled here, and everything else
        goes to the subscriber stream. A notification or message of the wrong
        shape is logged and dropped.
        """
        notification = push_from_frame(payload)
        if notification is None:
            logger.warning("dropping malformed push frame primitive=%s", self.name)
            return
        message_type = notification.message_type

        # Check for shutdown signal - SDK handles this internally
        if message_type == "system.shutdown":
            shutdown_kind = extract_shutdown_kind(notification.payload)
            logger.info(
                "received shutdown signal primitive=%s kind=%s",
                self.name,
                shutdown_kind if shutdown_kind is not None else "unknown",
            )

            # Close stream if shutdown is for this primitive's kind
            if shutdown_kind == self.primitive_kind.lower() and self._message_stream is not None:
                logger.info(
                    "shutting down (engine requested) primitive=%s",
                    self.name,
                )
                self._message_stream.close()
                self._message_stream = None
            # Don't forward system.shutdown to user - it's internal
            return

        # Skip the transport's own envelope broadcasts, which only a "*"
        # subscription ever sees. The message inside each one arrives
        # separately under its own Emergent message type.
        if not is_emergent_message_type(message_type):
            logger.debug(
                "skipping non-Emergent IPC broadcast primitive=%s message_type=%s",
                self.name,
                message_type,
            )
            return

        # The notification.payload IS the serialized EmergentMessage
        wire_message = wire_message_from_push(notification.payload)
        if wire_message is None:
            logger.warning(
                "dropping push with a malformed message primitive=%s message_type=%s",
                self.name,
                message_type,
            )
            return
        correlation_id = wire_message.correlation_id

        # Handle system.response.topology messages
        if message_type == "system.response.topology":
            topology_pending = (
                self._pending_topology_requests.pop(correlation_id, None)
                if correlation_id
                else None
            )
            if topology_pending is not None:
                if topology_pending.timer is not None:
                    topology_pending.timer.cancel()
                if not topology_pending.future.done():
                    topology_pending.future.set_result(topology_from_payload(wire_message.payload))
            return  # Don't forward to message stream

        # Handle system.response.subscriptions messages
        if message_type == "system.response.subscriptions":
            subscriptions_pending = (
                self._pending_subscriptions_requests.pop(correlation_id, None)
                if correlation_id
                else None
            )
            if subscriptions_pending is not None:
                if subscriptions_pending.timer is not None:
                    subscriptions_pending.timer.cancel()
                if not subscriptions_pending.future.done():
                    subscriptions_pending.future.set_result(
                        subscribes_from_payload(wire_message.payload)
                    )
            return  # Don't forward to message stream

        if self._message_stream is None:
            return

        logger.debug(
            "received message primitive=%s message_type=%s",
            self.name,
            message_type,
        )
        message = EmergentMessage.from_wire(wire_message)

        # Auto-unwrap exec-source stdout payloads when enabled
        if self._unwrap_stdout and not message.message_type.startswith("system."):
            message = message.unwrap_stdout()

        self._message_stream.push(message)

    def _on_stream_close(self, stream: MessageStream) -> None:
        """
        Forget a stream that closed, while it is still the registered one.

        A later subscribe may have replaced it by now, and that stream stays.
        """
        if self._message_stream is stream:
            self._message_stream = None
