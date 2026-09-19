package emergent

import "sync"

const defaultStreamBufferSize = 256

// MessageStream delivers messages from subscriptions via a Go channel.
//
// Use the C() method to get the receive-only channel for iteration:
//
//	for msg := range stream.C() {
//	    fmt.Println(msg.MessageType, msg.Payload)
//	}
//
// The channel is closed when the stream ends (engine shutdown, explicit close, or disconnect).
type MessageStream struct {
	ch      chan *EmergentMessage
	closed  bool
	mu      sync.Mutex
	onClose func()
}

func newMessageStream(bufSize int, onClose func()) *MessageStream {
	if bufSize <= 0 {
		bufSize = defaultStreamBufferSize
	}
	return &MessageStream{
		ch:      make(chan *EmergentMessage, bufSize),
		onClose: onClose,
	}
}

// C returns the receive-only channel for consuming messages.
// The channel is closed when the stream ends.
func (s *MessageStream) C() <-chan *EmergentMessage {
	return s.ch
}

// Next blocks until the next message arrives or the stream closes.
// Returns nil when the stream is closed.
func (s *MessageStream) Next() *EmergentMessage {
	msg, ok := <-s.ch
	if !ok {
		return nil
	}
	return msg
}

// TryNext returns the next message without blocking, or nil if none available.
func (s *MessageStream) TryNext() *EmergentMessage {
	select {
	case msg, ok := <-s.ch:
		if !ok {
			return nil
		}
		return msg
	default:
		return nil
	}
}

// Close closes the stream. Further pushes are discarded.
//
// The onClose callback runs after s.mu is released. The client's callback
// takes c.mu, and the read loop calls push with c.mu held, so running the
// callback under s.mu would order the two locks both ways round.
func (s *MessageStream) Close() {
	s.mu.Lock()
	if s.closed {
		s.mu.Unlock()
		return
	}
	s.closed = true
	close(s.ch)
	s.mu.Unlock()

	if s.onClose != nil {
		s.onClose()
	}
}

// IsClosed returns whether the stream has been closed.
func (s *MessageStream) IsClosed() bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.closed
}

// Pending returns the number of messages waiting in the channel buffer.
func (s *MessageStream) Pending() int {
	return len(s.ch)
}

// push sends a message to the stream. Dropped if stream is closed.
//
// s.mu is held across the send so Close cannot close the channel between the
// closed check and the send, which would panic. The send never blocks, so the
// lock is held only briefly.
func (s *MessageStream) push(msg *EmergentMessage) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		return
	}

	select {
	case s.ch <- msg:
	default:
		// Buffer full: drop the message to prevent blocking the read loop
	}
}
