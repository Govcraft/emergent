"""
Emergent Python SDK - Event-driven workflow platform client.

Three primitives for building Emergent clients:
- EmergentSource: Publish messages (Sources can only publish)
- EmergentHandler: Subscribe and publish (Handlers do both)
- EmergentSink: Subscribe only (Sinks consume messages)

Example (Simple Sink - 3 lines):
    >>> from emergent import EmergentSink
    >>>
    >>> async for msg in EmergentSink.messages("my_sink", ["timer.tick"]):
    ...     print(msg.payload)

Example (Handler with causation):
    >>> from emergent import EmergentHandler, create_message
    >>>
    >>> async with await EmergentHandler.connect("processor") as handler:
    ...     async with await handler.subscribe(["raw.event"]) as stream:
    ...         async for msg in stream:
    ...             await handler.publish(
    ...                 create_message("processed.event")
    ...                 .caused_by(msg.id)
    ...                 .payload({"processed": True})
    ...             )
"""

__version__ = "0.1.0"

# Types
# Protocol utilities (for advanced users)
from emergent._client import get_socket_path, socket_exists
from emergent._protocol import Format, generate_message_id

# Errors
from emergent.errors import (
    ConnectionError,
    DiscoveryError,
    DisposedError,
    EmergentError,
    ProtocolError,
    PublishError,
    SocketNotFoundError,
    SubscriptionError,
    TimeoutError,
    ValidationError,
)

# Client Primitives
from emergent.handler import EmergentHandler

# Message Building
from emergent.message import (
    MessageBuilder,
    create_message,
)
from emergent.sink import EmergentSink
from emergent.source import EmergentSource

# Stream
from emergent.stream import MessageStream
from emergent.types import (
    DiscoveryInfo,
    EmergentMessage,
    PrimitiveInfo,
)

# Helpers
from emergent.helpers import (
    HelperError,
    run_handler,
    run_sink,
    run_source,
)

__all__ = [
    "ConnectionError",
    "DiscoveryError",
    "DiscoveryInfo",
    "DisposedError",
    # Errors
    "EmergentError",
    "EmergentHandler",
    # Types
    "EmergentMessage",
    "EmergentSink",
    # Primitives
    "EmergentSource",
    "Format",
    "HelperError",
    "MessageBuilder",
    # Stream
    "MessageStream",
    "PrimitiveInfo",
    "ProtocolError",
    "PublishError",
    "SocketNotFoundError",
    "SubscriptionError",
    "TimeoutError",
    "ValidationError",
    # Version
    "__version__",
    # Message building
    "create_message",
    "generate_message_id",
    # Utilities
    "get_socket_path",
    # Helpers
    "run_handler",
    "run_sink",
    "run_source",
    "socket_exists",
]
