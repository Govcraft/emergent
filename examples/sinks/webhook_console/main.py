#!/usr/bin/env python3
"""Simple webhook console sink - subscribes only to webhook.received."""

import asyncio
import os

from emergent import EmergentSink
from emergent._protocol import Format


async def main() -> None:
    """Subscribe to webhook.received and display messages in green."""
    name = os.environ.get("EMERGENT_NAME", "webhook_console")

    async for msg in EmergentSink.messages(name, ["webhook.received"], format_=Format.MSGPACK):
        print(f"\033[32mWebhook says: {msg.payload.get('message', 'no message')}\033[0m", flush=True)


if __name__ == "__main__":
    asyncio.run(main())
