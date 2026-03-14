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

// TryDecodeFrame tries to decode a frame from a buffer.
// Returns nil if the buffer does not contain a complete frame.
func TryDecodeFrame(buffer []byte) (*DecodedFrame, error) {
	if len(buffer) < HeaderSize {
		return nil, nil // Not enough data for header
	}

	payloadLen := binary.BigEndian.Uint32(buffer[0:4])
	if int(payloadLen) > MaxFrameSize {
		return nil, &ProtocolError{Msg: fmt.Sprintf("frame too large: %d bytes", payloadLen)}
	}

	totalLen := HeaderSize + int(payloadLen)
	if len(buffer) < totalLen {
		return nil, nil // Not enough data for full frame
	}

	version := buffer[4]
	if version != ProtocolVersion {
		return nil, &ProtocolError{
			Msg: fmt.Sprintf("unsupported protocol version: %d (expected %d)", version, ProtocolVersion),
		}
	}

	msgType := buffer[5]
	format := buffer[6]
	payloadBytes := buffer[HeaderSize:totalLen]

	var payload any
	var err error

	switch format {
	case FormatMsgPack:
		err = msgpack.Unmarshal(payloadBytes, &payload)
	case FormatJSON:
		err = json.Unmarshal(payloadBytes, &payload)
	default:
		return nil, &ProtocolError{Msg: fmt.Sprintf("unknown format: %d", format)}
	}
	if err != nil {
		return nil, &ProtocolError{Msg: fmt.Sprintf("deserialization error: %v", err)}
	}

	return &DecodedFrame{
		MsgType:       msgType,
		Format:        format,
		Payload:       payload,
		BytesConsumed: totalLen,
	}, nil
}
