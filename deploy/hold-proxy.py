#!/usr/bin/env python3
"""Hold proxy in front of the dumpstar container (sp-slm2ib).

Listens on the origin port the Cloudflare tunnel targets and pipes each connection to the
container's backend port. A connection that arrives while the backend is down (a deploy
restarting it) is HELD, retrying the upstream connect until it answers or --hold seconds
pass, instead of being refused: the tunnel then sees a slow response, never a 502.

    hold-proxy.py --listen 127.0.0.1:8091 --upstream 127.0.0.1:18091 [--hold 60] [--log FILE]

Every held connection is logged (`held <seconds>`) so a deploy can report its window.
"""
import argparse
import asyncio
import sys
import time


def addr(s):
    host, _, port = s.rpartition(":")
    return host, int(port)


async def pipe(reader, writer):
    try:
        while True:
            data = await reader.read(65536)
            if not data:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, asyncio.CancelledError):
        pass
    finally:
        try:
            writer.close()
        except Exception:
            pass


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--listen", required=True)
    ap.add_argument("--upstream", required=True)
    ap.add_argument("--hold", type=float, default=60.0)
    ap.add_argument("--retry", type=float, default=0.05)
    args = ap.parse_args()
    lhost, lport = addr(args.listen)
    uhost, uport = addr(args.upstream)

    async def handle(creader, cwriter):
        start = time.monotonic()
        deadline = start + args.hold
        while True:
            try:
                ureader, uwriter = await asyncio.open_connection(uhost, uport)
                break
            except OSError:
                if time.monotonic() >= deadline:
                    print(f"refused after {args.hold:.0f}s: upstream never answered", flush=True)
                    cwriter.close()
                    return
                await asyncio.sleep(args.retry)
        waited = time.monotonic() - start
        if waited > 0.2:
            print(f"held {waited:.2f}s", flush=True)
        await asyncio.gather(pipe(creader, uwriter), pipe(ureader, cwriter))

    server = await asyncio.start_server(handle, lhost, lport, backlog=1024)
    print(f"hold-proxy {args.listen} -> {args.upstream} (hold {args.hold:.0f}s)", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        sys.exit(0)
