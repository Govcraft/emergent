package emergent

import (
	"testing"
	"time"
)

func TestMessageStream_PushAndReceive(t *testing.T) {
	stream := newMessageStream(10, nil)
	defer stream.Close()

	msg, _ := NewMessage("test.event")
	stream.push(msg)

	received := stream.Next()
	if received == nil {
		t.Fatal("expected message")
	}
	if string(received.MessageType) != "test.event" {
		t.Errorf("expected test.event, got: %s", received.MessageType)
	}
}

func TestMessageStream_Close(t *testing.T) {
	stream := newMessageStream(10, nil)
	stream.Close()

	if !stream.IsClosed() {
		t.Error("expected stream to be closed")
	}

	// Next should return nil after close
	msg := stream.Next()
	if msg != nil {
		t.Error("expected nil after close")
	}
}

func TestMessageStream_CloseCallback(t *testing.T) {
	called := false
	stream := newMessageStream(10, func() {
		called = true
	})
	stream.Close()

	if !called {
		t.Error("expected close callback to be called")
	}
}

func TestMessageStream_DoubleClose(t *testing.T) {
	callCount := 0
	stream := newMessageStream(10, func() {
		callCount++
	})
	stream.Close()
	stream.Close() // Should not panic or call callback twice

	if callCount != 1 {
		t.Errorf("expected callback called once, got: %d", callCount)
	}
}

func TestMessageStream_TryNext(t *testing.T) {
	stream := newMessageStream(10, nil)
	defer stream.Close()

	// Empty stream
	msg := stream.TryNext()
	if msg != nil {
		t.Error("expected nil from empty stream")
	}

	// After push
	m, _ := NewMessage("test.event")
	stream.push(m)

	msg = stream.TryNext()
	if msg == nil {
		t.Fatal("expected message")
	}
}

func TestMessageStream_Channel(t *testing.T) {
	stream := newMessageStream(10, nil)

	msg, _ := NewMessage("test.event")
	stream.push(msg)
	stream.Close()

	count := 0
	for range stream.C() {
		count++
	}
	if count != 1 {
		t.Errorf("expected 1 message, got: %d", count)
	}
}

func TestMessageStream_Pending(t *testing.T) {
	stream := newMessageStream(10, nil)
	defer stream.Close()

	if stream.Pending() != 0 {
		t.Errorf("expected 0 pending, got: %d", stream.Pending())
	}

	msg, _ := NewMessage("test.event")
	stream.push(msg)

	if stream.Pending() != 1 {
		t.Errorf("expected 1 pending, got: %d", stream.Pending())
	}
}

func TestMessageStream_PushAfterClose(t *testing.T) {
	stream := newMessageStream(10, nil)
	stream.Close()

	// Should not panic
	msg, _ := NewMessage("test.event")
	stream.push(msg)
}

func TestMessageStream_BufferFull(t *testing.T) {
	stream := newMessageStream(2, nil)
	defer stream.Close()

	// Fill buffer
	m1, _ := NewMessage("test.event")
	m2, _ := NewMessage("test.event")
	m3, _ := NewMessage("test.event")
	stream.push(m1)
	stream.push(m2)

	// This should be dropped (buffer full), not block
	done := make(chan struct{})
	go func() {
		stream.push(m3)
		close(done)
	}()

	select {
	case <-done:
		// Good - push returned without blocking
	case <-time.After(time.Second):
		t.Fatal("push blocked on full buffer")
	}

	if stream.Pending() != 2 {
		t.Errorf("expected 2 pending (third dropped), got: %d", stream.Pending())
	}
}
