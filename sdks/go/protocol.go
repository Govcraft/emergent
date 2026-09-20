package emergent

import (
	"encoding/binary"
	"encoding/json"
	"fmt"

	"github.com/vmihailenco/msgpack/v5"
)

// Protocol constants matching acton-reactive IPC.
const (
	ProtocolVersion byte = 0x02
	HeaderSize      int  = 7
	MaxFrameSize    int  = 16 * 1024 * 1024 // 16 MiB
)

// IPC message type constants.
const (
	MsgTypeRequest     byte = 0x01
	MsgTypeResponse    byte = 0x02
	MsgTypeError       byte = 0x03
	MsgTypeHeartbeat   byte = 0x04
	MsgTypePush        byte = 0x05
	MsgTypeSubscribe   byte = 0x06
	MsgTypeUnsubscribe byte = 0x07
	MsgTypeDiscover    byte = 0x08
	MsgTypeStream      byte = 0x09

	// MsgTypeSubscribePatterns subscribes to IPC prefix patterns.
	// Requires an engine newer than 0.10.10.
	MsgTypeSubscribePatterns byte = 0x0a
	// MsgTypeUnsubscribePatterns unsubscribes from IPC prefix patterns.
	// Requires an engine newer than 0.10.10.
	MsgTypeUnsubscribePatterns byte = 0x0b
)

// Serialization format constants.
const (
	FormatJSON    byte = 0x01
	FormatMsgPack byte = 0x02
)

// DecodedFrame represents a decoded protocol frame.
type DecodedFrame struct {
	MsgType       byte
	Format        byte
	Payload       any
	BytesConsumed int
}

// EncodeFrame encodes a payload into a protocol frame.
//
// Frame structure:
//   - [0-3]: Payload length (big-endian u32)
//   - [4]: Protocol version
//   - [5]: Message type
//   - [6]: Serialization format
//   - [7+]: Payload bytes
func EncodeFrame(msgType byte, payload any, format byte) ([]byte, error) {
	var payloadBytes []byte
	var err error

	switch format {
	case FormatMsgPack:
		payloadBytes, err = msgpack.Marshal(payload)
	case FormatJSON:
		payloadBytes, err = json.Marshal(payload)
	default:
		return nil, &ProtocolError{Msg: fmt.Sprintf("unsupported format: %d", format)}
	}
	if err != nil {
		return nil, fmt.Errorf("serialization error: %w", err)
	}

	payloadLen := len(payloadBytes)
	if payloadLen > MaxFrameSize {
		return nil, &ProtocolError{
			Msg: fmt.Sprintf("payload too large: %d bytes (max: %d)", payloadLen, MaxFrameSize),
		}
	}

	frame := make([]byte, HeaderSize+payloadLen)
	binary.BigEndian.PutUint32(frame[0:4], uint32(payloadLen))
	frame[4] = ProtocolVersion
	frame[5] = msgType
	frame[6] = format
	copy(frame[HeaderSize:], payloadBytes)

	return frame, nil
}

// frameStepKind says what the front of a read buffer holds.
type frameStepKind int

const (
	// frameIncomplete: not a whole frame yet, so wait for more bytes.
	frameIncomplete frameStepKind = iota
	// frameDecoded: a decoded frame.
	frameDecoded
	// frameBadBody: a whole frame whose body does not decode. Its length is
	// known, so the reader drops bytesConsumed bytes and carries on.
	frameBadBody
	// frameBadFraming: a header that cannot be trusted, so the reader cannot
	// tell where the next frame starts.
	frameBadFraming
)

// frameStep is the result of reading the front of a read buffer.
type frameStep struct {
	kind frameStepKind
	// frame is set for frameDecoded.
	frame *DecodedFrame
	// msgType and bytesConsumed are set for frameBadBody.
	msgType       byte
	bytesConsumed int
	// reason is set for frameBadBody and frameBadFraming.
	reason string
}

// nextFrame reads the frame at the front of a buffer. A body that does not
// decode is told apart from a header that cannot be trusted, because only the
// first leaves the reader able to find the next frame.
func nextFrame(buffer []byte) frameStep {
	if len(buffer) < HeaderSize {
		return frameStep{kind: frameIncomplete} // Not enough data for header
	}

	payloadLen := binary.BigEndian.Uint32(buffer[0:4])
	if int(payloadLen) > MaxFrameSize {
		return frameStep{kind: frameBadFraming, reason: fmt.Sprintf("frame too large: %d bytes", payloadLen)}
	}

	totalLen := HeaderSize + int(payloadLen)
	if len(buffer) < totalLen {
		return frameStep{kind: frameIncomplete} // Not enough data for full frame
	}

	version := buffer[4]
	if version != ProtocolVersion {
		return frameStep{
			kind:   frameBadFraming,
			reason: fmt.Sprintf("unsupported protocol version: %d (expected %d)", version, ProtocolVersion),
		}
	}

	msgType := buffer[5]
	format := buffer[6]
	payloadBytes := buffer[HeaderSize:totalLen]

	// A heartbeat is a bare header. Its body is empty, which neither format
	// can decode, so it is returned with a nil payload.
	if payloadLen == 0 {
		return frameStep{
			kind:  frameDecoded,
			frame: &DecodedFrame{MsgType: msgType, Format: format, BytesConsumed: totalLen},
		}
	}

	badBody := func(reason string) frameStep {
		return frameStep{kind: frameBadBody, msgType: msgType, bytesConsumed: totalLen, reason: reason}
	}

	var payload any
	var err error

	switch format {
	case FormatMsgPack:
		err = msgpack.Unmarshal(payloadBytes, &payload)
	case FormatJSON:
		err = json.Unmarshal(payloadBytes, &payload)
	default:
		return badBody(fmt.Sprintf("unknown format: %d", format))
	}
	if err != nil {
		return badBody(fmt.Sprintf("deserialization error: %v", err))
	}

	return frameStep{
		kind: frameDecoded,
		frame: &DecodedFrame{
			MsgType:       msgType,
			Format:        format,
			Payload:       payload,
			BytesConsumed: totalLen,
		},
	}
}

// TryDecodeFrame tries to decode a frame from a buffer.
// Returns nil if the buffer does not contain a complete frame.
func TryDecodeFrame(buffer []byte) (*DecodedFrame, error) {
	step := nextFrame(buffer)
	switch step.kind {
	case frameDecoded:
		return step.frame, nil
	case frameBadBody, frameBadFraming:
		return nil, &ProtocolError{Msg: step.reason}
	default:
		return nil, nil
	}
}
