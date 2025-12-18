"""
Error types for the Emergent client SDK.

All errors include a `code` property for programmatic error handling.
"""

from __future__ import annotations


class EmergentError(Exception):
    """
    Base error class for all Emergent errors.

    All errors include a `code` property for programmatic error handling.

    Attributes:
        code: Error code for programmatic handling
        message: Human-readable error message
    """

    code: str = "UNKNOWN"

    def __init__(self, message: str, code: str | None = None) -> None:
        super().__init__(message)
        if code is not None:
            self.code = code


class ConnectionError(EmergentError):
    """
    Error thrown when connection to the engine fails.

    Common causes:
    - Engine not running
    - Socket path incorrect
    - Permission denied
    """

    code = "CONNECTION_FAILED"

    def __init__(self, message: str) -> None:
        super().__init__(message, self.code)


class SocketNotFoundError(EmergentError):
    """Error thrown when the socket is not found."""

    code = "SOCKET_NOT_FOUND"

    def __init__(self, socket_path: str) -> None:
        super().__init__(
            f"Socket not found at {socket_path}. Is the Emergent engine running?",
            self.code,
        )
        self.socket_path = socket_path


class TimeoutError(EmergentError):
    """Error thrown when a request times out."""

    code = "TIMEOUT"

    def __init__(
        self, message: str = "Operation timed out", timeout: float = 0.0
    ) -> None:
        super().__init__(message, self.code)
        self.timeout = timeout


class ProtocolError(EmergentError):
    """Error thrown when there's a protocol-level error."""

    code = "PROTOCOL_ERROR"

    def __init__(self, message: str) -> None:
        super().__init__(message, self.code)


class SubscriptionError(EmergentError):
    """Error thrown when subscription fails."""

    code = "SUBSCRIPTION_FAILED"

    def __init__(
        self, message: str, message_types: list[str] | None = None
    ) -> None:
        super().__init__(message, self.code)
        self.message_types = message_types or []


class PublishError(EmergentError):
    """Error thrown when publishing fails."""

    code = "PUBLISH_FAILED"

    def __init__(self, message: str, message_type: str = "") -> None:
        super().__init__(message, self.code)
        self.message_type = message_type


class DiscoveryError(EmergentError):
    """Error thrown when discovery fails."""

    code = "DISCOVERY_FAILED"

    def __init__(self, message: str) -> None:
        super().__init__(message, self.code)


class DisposedError(EmergentError):
    """Error thrown when the client has been disposed."""

    code = "DISPOSED"

    def __init__(self, client_type: str) -> None:
        super().__init__(
            f"{client_type} has been disposed and cannot be used",
            self.code,
        )


class ValidationError(EmergentError):
    """Error thrown when validation fails."""

    code = "VALIDATION_ERROR"

    def __init__(self, message: str, field: str) -> None:
        super().__init__(message, self.code)
        self.field = field
