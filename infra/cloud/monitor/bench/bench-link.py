"""Local TCP link with bounded queues, 30 Mbit/s, and symmetric delay."""
import asyncio
import sys

async def pipe(reader, writer, delay):
    queue = asyncio.Queue(32)
    loop = asyncio.get_running_loop()
    async def schedule():
        available = loop.time()
        while True:
            data = await reader.read(32768)
            available = max(available, loop.time()) + len(data) * 8 / 30_000_000
            await queue.put((available + delay, data))
            if not data:
                return
    producer = asyncio.create_task(schedule())
    try:
        while True:
            when, data = await queue.get()
            await asyncio.sleep(max(0, when-loop.time()))
            if not data:
                break
            writer.write(data)
            await writer.drain()
    finally:
        producer.cancel()
        writer.close()
        await asyncio.gather(producer, return_exceptions=True)

async def main():
    delay = int(sys.argv[1]) / 2000
    async def connect(client, downstream):
        upstream, server = await asyncio.open_connection('127.0.0.1',6901)
        await asyncio.gather(pipe(client,server,delay),pipe(upstream,downstream,delay),return_exceptions=True)
    listener = await asyncio.start_server(connect,'127.0.0.1',7902)
    async with listener:
        await listener.serve_forever()
asyncio.run(main())
