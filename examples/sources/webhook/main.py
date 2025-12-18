#!/usr/bin/env python3
"""HTTP Webhook Source - Listens for HTTP requests and publishes to Emergent."""

import argparse
import asyncio
import os
import signal

from aiohttp import web

from emergent import EmergentSource
from emergent._protocol import Format

source: EmergentSource | None = None


async def handle_webhook(request: web.Request) -> web.Response:
    """Handle incoming webhook POST requests."""
    try:
        body = await request.json()
    except Exception:
        body = {}

    message_text = body.get("message", "Hello World")

    if source:
        await source.publish("webhook.received", {"message": message_text})

    return web.json_response({"status": "ok", "message": message_text})


async def main() -> None:
    global source

    parser = argparse.ArgumentParser(description="HTTP webhook source for Emergent")
    parser.add_argument("--host", default="127.0.0.1", help="Host to bind to")
    parser.add_argument("--port", type=int, default=8080, help="Port to listen on")
    args = parser.parse_args()

    name = os.environ.get("EMERGENT_NAME", "webhook")
    source = await EmergentSource.connect(name, format_=Format.MSGPACK)

    app = web.Application()
    app.router.add_post("/webhook", handle_webhook)

    runner = web.AppRunner(app)
    await runner.setup()
    site = web.TCPSite(runner, args.host, args.port)
    await site.start()

    # Wait for shutdown signal
    stop_event = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGTERM, signal.SIGINT):
        loop.add_signal_handler(sig, stop_event.set)

    await stop_event.wait()

    await runner.cleanup()
    await source.disconnect()


if __name__ == "__main__":
    asyncio.run(main())
