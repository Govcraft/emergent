/**
 * Observability logging for the Emergent SDK.
 *
 * Logs to `~/.local/share/emergent/<name>/primitive.log` by default so
 * the console stays clean for child-process output.
 *
 * Set `EMERGENT_LOG=stderr` to log to stderr instead (for debugging).
 * Set `EMERGENT_LOG=off` to disable logging entirely.
 *
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

/** Resolve the XDG data directory for a primitive. */
function resolveLogDir(name: string): string {
  try {
    const xdgData = Deno.env.get("XDG_DATA_HOME") ??
      `${Deno.env.get("HOME")}/.local/share`;
    return `${xdgData}/emergent/${name}`;
  } catch {
    return `.`;
  }
}

/** Format a log line with timestamp and level. */
function formatLine(
  level: string,
  name: string,
  msg: string,
  data?: Record<string, unknown>,
): string {
  const ts = new Date().toISOString();
  const suffix = data !== undefined ? ` ${JSON.stringify(data)}` : "";
  return `${ts} ${level} [emergent:${name}] ${msg}${suffix}\n`;
}

/**
 * Structured logger for Emergent SDK components.
 *
 * Reads its level and destination from the `EMERGENT_LOG` environment variable.
 * Default: log to file. `EMERGENT_LOG=stderr`: log to console.
 */
export class Logger {
  readonly #name: string;
  readonly #level: LogLevel;
  readonly #file: Deno.FsFile | null;
  readonly #useStderr: boolean;
  readonly #encoder: TextEncoder;

  constructor(name: string) {
    this.#name = name;
    this.#encoder = new TextEncoder();

    let envValue = "";
    try {
      envValue = Deno.env.get("EMERGENT_LOG") ?? "";
    } catch {
      // Permission denied or env not available
    }

    const wantsStderr = envValue.toLowerCase() === "stderr";
    this.#useStderr = wantsStderr;

    if (wantsStderr) {
      this.#level = LogLevel.INFO;
      this.#file = null;
    } else if (envValue) {
      this.#level = parseLogLevel(envValue);
      this.#file = this.#openLogFile(name);
    } else {
      this.#level = LogLevel.INFO;
      this.#file = this.#openLogFile(name);
    }
  }

  #openLogFile(name: string): Deno.FsFile | null {
    try {
      const logDir = resolveLogDir(name);
      Deno.mkdirSync(logDir, { recursive: true });
      return Deno.openSync(`${logDir}/primitive.log`, {
        create: true,
        append: true,
      });
    } catch {
      // Can't open log file — go silent
      return null;
    }
  }

  #write(level: string, msg: string, data?: Record<string, unknown>): void {
    const line = formatLine(level, this.#name, msg, data);

    if (this.#useStderr) {
      Deno.stderr.writeSync(this.#encoder.encode(line));
      return;
    }

    if (this.#file) {
      try {
        this.#file.writeSync(this.#encoder.encode(line));
      } catch {
        // Silently ignore write errors
      }
    }
  }

  /** Log a debug-level message. */
  debug(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.DEBUG) {
      this.#write("DEBUG", msg, data);
    }
  }

  /** Log an info-level message. */
  info(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.INFO) {
      this.#write("INFO", msg, data);
    }
  }

  /** Log a warn-level message. */
  warn(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.WARN) {
      this.#write("WARN", msg, data);
    }
  }

  /** Log an error-level message. */
  error(msg: string, data?: Record<string, unknown>): void {
    if (this.#level <= LogLevel.ERROR) {
      this.#write("ERROR", msg, data);
    }
  }
}

// ============================================================================
// Factory
// ============================================================================

/**
 * Create a new Logger instance for the given component name.
 *
 * @param name - Component name (used in log file path and log prefix)
 * @returns A configured Logger instance
 */
export function createLogger(name: string): Logger {
  return new Logger(name);
}
