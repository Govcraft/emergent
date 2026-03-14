package emergent

import (
	"fmt"
	"io"
	"log"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// LogLevel represents the severity of a log message.
type LogLevel int

const (
	LogLevelDebug LogLevel = iota
	LogLevelInfo
	LogLevelWarn
	LogLevelError
	LogLevelOff
)

var logLevelNames = map[LogLevel]string{
	LogLevelDebug: "DEBUG",
	LogLevelInfo:  "INFO",
	LogLevelWarn:  "WARN",
	LogLevelError: "ERROR",
}

// Logger provides structured logging for SDK components.
//
// Logs to ~/.local/share/emergent/<name>/primitive.log by default.
// Set EMERGENT_LOG=stderr to log to stderr.
// Set EMERGENT_LOG=off to disable logging.
type Logger struct {
	name   string
	level  LogLevel
	logger *log.Logger
	closer io.Closer // non-nil if we own the underlying file
}

func newLogger(name string) *Logger {
	envVal := os.Getenv("EMERGENT_LOG")
	if envVal == "" {
		envVal = os.Getenv("RUST_LOG")
	}

	level := LogLevelInfo
	var writer io.Writer
	var closer io.Closer

	switch strings.ToLower(envVal) {
	case "stderr":
		writer = os.Stderr
	case "off":
		level = LogLevelOff
		writer = io.Discard
	case "debug":
		level = LogLevelDebug
		writer, closer = createLogFile(name)
	case "warn":
		level = LogLevelWarn
		writer, closer = createLogFile(name)
	case "error":
		level = LogLevelError
		writer, closer = createLogFile(name)
	default:
		writer, closer = createLogFile(name)
	}

	return &Logger{
		name:   name,
		level:  level,
		logger: log.New(writer, "", 0),
		closer: closer,
	}
}

func createLogFile(name string) (io.Writer, io.Closer) {
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		home, err := os.UserHomeDir()
		if err != nil {
			return os.Stderr, nil
		}
		dataHome = filepath.Join(home, ".local", "share")
	}

	logDir := filepath.Join(dataHome, "emergent", name)
	if err := os.MkdirAll(logDir, 0o755); err != nil {
		return os.Stderr, nil
	}

	logPath := filepath.Join(logDir, "primitive.log")
	f, err := os.OpenFile(logPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o644)
	if err != nil {
		return os.Stderr, nil
	}
	return f, f
}

// Close closes the underlying log file, if any.
func (l *Logger) Close() {
	if l.closer != nil {
		l.closer.Close()
		l.closer = nil
	}
}

func (l *Logger) log(level LogLevel, msg string, keysAndValues ...any) {
	if l.level > level {
		return
	}

	levelName := logLevelNames[level]
	timestamp := time.Now().Format("2006-01-02T15:04:05.000Z07:00")

	var sb strings.Builder
	sb.WriteString(fmt.Sprintf("%s %s %s: %s", timestamp, levelName, l.name, msg))

	for i := 0; i+1 < len(keysAndValues); i += 2 {
		sb.WriteString(fmt.Sprintf(" %v=%v", keysAndValues[i], keysAndValues[i+1]))
	}

	l.logger.Println(sb.String())
}

// Debug logs a debug message.
func (l *Logger) Debug(msg string, keysAndValues ...any) {
	l.log(LogLevelDebug, msg, keysAndValues...)
}

// Info logs an info message.
func (l *Logger) Info(msg string, keysAndValues ...any) {
	l.log(LogLevelInfo, msg, keysAndValues...)
}

// Warn logs a warning message.
func (l *Logger) Warn(msg string, keysAndValues ...any) {
	l.log(LogLevelWarn, msg, keysAndValues...)
}

// Error logs an error message.
func (l *Logger) Error(msg string, keysAndValues ...any) {
	l.log(LogLevelError, msg, keysAndValues...)
}
