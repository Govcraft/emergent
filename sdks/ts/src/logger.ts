/**
 * Observability logging for the Emergent SDK.
 * @module
 */

// ============================================================================
// Log Levels
// ============================================================================

/**
 * Log levels for the Emergent SDK logger.
 *
 * Set via the `EMERGENT_LOG` environment variable (case-insensitive).
 * Defaults to INFO when not set or when env permission is denied.
 */
export enum LogLevel {
  DEBUG = 0,
  INFO = 1,
  WARN = 2,
  ERROR = 3,
  OFF = 4,
}

/**
 * Parse a string into a LogLevel (case-insensitive).
 *
 * @param str - The string to parse (e.g., "debug", "INFO", "warn")
 * @returns The corresponding LogLevel, defaults to INFO for unrecognized values
 */
export function parseLogLevel(str: string): LogLevel {
  switch (str.toLowerCase()) {
    case "debug":
      return LogLevel.DEBUG;
    case "info":
      return LogLevel.INFO;
    case "warn":
      return LogLevel.WARN;
    case "error":
      return LogLevel.ERROR;
    case "off":
      return LogLevel.OFF;
    default:
      return LogLevel.INFO;
  }
}

// ============================================================================
// Logger
// ============================================================================

/**
 * Structured logger for Emergent SDK components.
 *
 * Reads its level from the `EMERGENT_LOG` environment variable.
 * Each log line is prefixed with `[emergent:<name>]` for easy filtering.
 */
export class Logger {
  readonly #name: string;
  readonly #level: LogLevel;

  constructor(name: string) {
    this.#name = name;

    let level = LogLevel.INFO;
    try {
      const envValue = Deno.env.get("EMERGENT_LOG");
      if (envValue) {
        level = parseLogLevel(envValue);
      }
    } catch {
      // Permission denied or env not available - use default
    }
    this.#level = level;
  }

  /**
   * Log a debug-level message.
   */
  debug(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.DEBUG) {
      if (data !== undefined) {
        console.debug(`[emergent:${this.#name}] ${msg}`, data);
      } else {
        console.debug(`[emergent:${this.#name}] ${msg}`);
      }
    }
  }

  /**
   * Log an info-level message.
   */
  info(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.INFO) {
      if (data !== undefined) {
        console.info(`[emergent:${this.#name}] ${msg}`, data);
      } else {
        console.info(`[emergent:${this.#name}] ${msg}`);
      }
    }
  }

  /**
   * Log a warn-level message.
   */
  warn(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.WARN) {
      if (data !== undefined) {
        console.warn(`[emergent:${this.#name}] ${msg}`, data);
      } else {
        console.warn(`[emergent:${this.#name}] ${msg}`);
      }
    }
  }

  /**
   * Log an error-level message.
   */
  error(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.ERROR) {
      if (data !== undefined) {
        console.error(`[emergent:${this.#name}] ${msg}`, data);
      } else {
        console.error(`[emergent:${this.#name}] ${msg}`);
      }
    }
  }
}

// ============================================================================
// Factory
// ============================================================================

/**
 * Create a new Logger instance for the given component name.
 *
 * @param name - Component name (appears as `[emergent:<name>]` in log output)
 * @returns A configured Logger instance
 */
export function createLogger(name: string): Logger {
  return new Logger(name);
}
