/**
 * Error types for the Emergent client SDK.
 * @module
 */

/**
 * Base error class for all Emergent errors.
 *
 * All errors include a `code` property for programmatic error handling.
 */
export class EmergentError extends Error {
  /** Error code for programmatic handling */
  readonly code: string;

  constructor(message: string, code = "UNKNOWN") {
    super(message);
    this.name = "EmergentError";
    this.code = code;
    // Maintain proper stack trace in V8
    if (Error.captureStackTrace) {
      Error.captureStackTrace(this, this.constructor);
    }
  }
}

/**
 * Error thrown when connection to the engine fails.
 *
 * Common causes:
 * - Engine not running
 * - Socket path incorrect
 * - Permission denied
 */
export class ConnectionError extends EmergentError {
  constructor(message: string) {
    super(message, "CONNECTION_FAILED");
    this.name = "ConnectionError";
  }
}

/**
 * Error thrown when the socket is not found.
 */
export class SocketNotFoundError extends EmergentError {
  /** The socket path that was not found */
  readonly socketPath: string;

  constructor(socketPath: string) {
    super(
      `Socket not found at ${socketPath}. Is the Emergent engine running?`,
      "SOCKET_NOT_FOUND",
    );
    this.name = "SocketNotFoundError";
    this.socketPath = socketPath;
  }
}

/**
 * Error thrown when a request times out.
 */
export class TimeoutError extends EmergentError {
  /** Timeout duration in milliseconds */
  readonly timeoutMs: number;

  constructor(message = "Operation timed out", timeoutMs = 0) {
    super(message, "TIMEOUT");
    this.name = "TimeoutError";
    this.timeoutMs = timeoutMs;
  }
}

/**
 * Error thrown when there's a protocol-level error.
 */
export class ProtocolError extends EmergentError {
  constructor(message: string) {
    super(message, "PROTOCOL_ERROR");
    this.name = "ProtocolError";
  }
}

/**
 * Error thrown when subscription fails.
 */
export class SubscriptionError extends EmergentError {
  /** The message types that failed to subscribe */
  readonly messageTypes: string[];

  constructor(message: string, messageTypes: string[] = []) {
    super(message, "SUBSCRIPTION_FAILED");
    this.name = "SubscriptionError";
    this.messageTypes = messageTypes;
  }
}

/**
 * Error thrown when publishing fails.
 */
export class PublishError extends EmergentError {
  /** The message type that failed to publish */
  readonly messageType: string;

  constructor(message: string, messageType = "") {
    super(message, "PUBLISH_FAILED");
    this.name = "PublishError";
    this.messageType = messageType;
  }
}

/**
 * Error thrown when discovery fails.
 */
export class DiscoveryError extends EmergentError {
  constructor(message: string) {
    super(message, "DISCOVERY_FAILED");
    this.name = "DiscoveryError";
  }
}

/**
 * Error thrown when the client has been disposed.
 */
export class DisposedError extends EmergentError {
  constructor(clientType: string) {
    super(`${clientType} has been disposed and cannot be used`, "DISPOSED");
    this.name = "DisposedError";
  }
}

/**
 * Error thrown when validation fails.
 */
export class ValidationError extends EmergentError {
  /** The field that failed validation */
  readonly field: string;

  constructor(message: string, field: string) {
    super(message, "VALIDATION_ERROR");
    this.name = "ValidationError";
    this.field = field;
  }
}
