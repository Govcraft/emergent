"""Tests for the stream module."""

import asyncio

import pytest

from emergent.stream import MessageStream
from emergent.types import EmergentMessage


def make_message(msg_type: str = "test.event", count: int = 0) -> EmergentMessage:
    """Create a test message."""
    return EmergentMessage(
        id=f"msg_{count}",
        message_type=msg_type,
        source="test",
        timestamp_ms=1234567890123,
        payload={"count": count},
    )


class TestMessageStream:
    """Tests for MessageStream."""

    def test_initial_state(self) -> None:
        """Test initial stream state."""
        stream = MessageStream()

        assert stream.closed is False
        assert stream.pending == 0

    def test_push_increments_pending(self) -> None:
        """Test that push increments pending count."""
        stream = MessageStream()

        stream.push(make_message(count=1))
        assert stream.pending == 1

        stream.push(make_message(count=2))
        assert stream.pending == 2

    def test_try_next_returns_message(self) -> None:
        """Test try_next returns message from queue."""
        stream = MessageStream()

        msg = make_message(count=1)
        stream.push(msg)

        result = stream.try_next()
        assert result is not None
        assert result.id == "msg_1"
        assert stream.pending == 0

    def test_try_next_returns_none_when_empty(self) -> None:
        """Test try_next returns None when queue is empty."""
        stream = MessageStream()

        result = stream.try_next()
        assert result is None

    def test_try_next_returns_none_when_closed(self) -> None:
        """Test try_next returns None when stream is closed."""
        stream = MessageStream()
        stream.close()

        result = stream.try_next()
        assert result is None

    @pytest.mark.asyncio
    async def test_next_returns_message(self) -> None:
        """Test next returns message from queue."""
        stream = MessageStream()

        msg = make_message(count=1)
        stream.push(msg)

        result = await stream.next()
        assert result is not None
        assert result.id == "msg_1"

    @pytest.mark.asyncio
    async def test_next_waits_for_message(self) -> None:
        """Test next blocks until message is available."""
        stream = MessageStream()

        async def push_later() -> None:
            await asyncio.sleep(0.01)
            stream.push(make_message(count=1))

        asyncio.create_task(push_later())
        result = await stream.next()

        assert result is not None
        assert result.id == "msg_1"

    @pytest.mark.asyncio
    async def test_next_returns_none_when_closed(self) -> None:
        """Test next returns None when stream is closed."""
        stream = MessageStream()
        stream.close()

        result = await stream.next()
        assert result is None

    @pytest.mark.asyncio
    async def test_next_returns_none_when_closed_while_waiting(self) -> None:
        """Test next returns None when stream is closed while waiting."""
        stream = MessageStream()

        async def close_later() -> None:
            await asyncio.sleep(0.01)
            stream.close()

        asyncio.create_task(close_later())
        result = await stream.next()

        assert result is None

    def test_close_sets_closed_flag(self) -> None:
        """Test that close sets the closed flag."""
        stream = MessageStream()

        stream.close()
        assert stream.closed is True

    def test_close_is_idempotent(self) -> None:
        """Test that close can be called multiple times."""
        stream = MessageStream()

        stream.close()
        stream.close()
        stream.close()

        assert stream.closed is True

    def test_push_ignored_when_closed(self) -> None:
        """Test that push is ignored when stream is closed."""
        stream = MessageStream()
        stream.close()

        stream.push(make_message(count=1))
        assert stream.pending == 0

    def test_on_close_callback(self) -> None:
        """Test that on_close callback is called."""
        called = []

        def callback() -> None:
            called.append(True)

        stream = MessageStream(on_close=callback)
        stream.close()

        assert len(called) == 1

    def test_on_close_callback_only_called_once(self) -> None:
        """Test that on_close callback is only called once."""
        called = []

        def callback() -> None:
            called.append(True)

        stream = MessageStream(on_close=callback)
        stream.close()
        stream.close()

        assert len(called) == 1

    @pytest.mark.asyncio
    async def test_async_iteration(self) -> None:
        """Test async iteration over messages."""
        stream = MessageStream()

        # Push some messages
        for i in range(3):
            stream.push(make_message(count=i))

        # Close after a delay
        async def close_later() -> None:
            await asyncio.sleep(0.01)
            stream.close()

        asyncio.create_task(close_later())

        # Collect messages
        messages = []
        async for msg in stream:
            messages.append(msg)

        assert len(messages) == 3
        assert [m.payload["count"] for m in messages] == [0, 1, 2]

    @pytest.mark.asyncio
    async def test_async_context_manager(self) -> None:
        """Test async context manager."""
        stream = MessageStream()
        stream.push(make_message(count=1))

        async with stream:
            msg = await stream.next()
            assert msg is not None
            assert msg.id == "msg_1"

        assert stream.closed is True

    @pytest.mark.asyncio
    async def test_async_context_manager_closes_on_exception(self) -> None:
        """Test that async context manager closes stream on exception."""
        stream = MessageStream()

        with pytest.raises(ValueError):
            async with stream:
                raise ValueError("test error")

        assert stream.closed is True

    def test_max_queue_size(self) -> None:
        """Test that queue respects max size."""
        stream = MessageStream(max_queue_size=3)

        for i in range(5):
            stream.push(make_message(count=i))

        assert stream.pending == 3
        # First two messages should have been dropped
        msg1 = stream.try_next()
        msg2 = stream.try_next()
        msg3 = stream.try_next()

        assert msg1 is not None and msg1.payload["count"] == 2
        assert msg2 is not None and msg2.payload["count"] == 3
        assert msg3 is not None and msg3.payload["count"] == 4

    def test_close_with_error(self) -> None:
        """Test close_with_error."""
        stream = MessageStream()
        stream.close_with_error()

        assert stream.closed is True
