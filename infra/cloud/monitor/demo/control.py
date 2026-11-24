"""Private HTTP/SSH demo: forward poof video and inject one controller's X11 input."""
import asyncio
import hmac
import os
import secrets
from pathlib import Path

from aiohttp import ClientSession, web
from Xlib import X, XK, display
from Xlib.ext import xtest

state = Path.home() / ".local/state/zuse-monitor"
os.umask(0o077)
control_key = secrets.token_urlsafe(32)
(state / "control-password").write_text(control_key + "\n")
base = "http://127.0.0.1:6901"
owner = asyncio.Lock()


async def video(request):
    path = request.path
    if path not in {"/", "/desktop-decoder.js", "/ready", "/v2/stream", "/stream"}:
        raise web.HTTPNotFound()
    headers = {}
    if path not in {"/", "/desktop-decoder.js"}:
        if not hmac.compare_digest(request.headers.get("Authorization", "").encode(),
                                   ("Bearer " + control_key).encode()):
            raise web.HTTPUnauthorized()
        headers["Authorization"] = "Bearer " + (state / "password").read_text().strip()
    async with request.app["client"].get(base + path, headers=headers) as upstream:
        if path == "/":
            html = (await upstream.text()).replace("View only", "Desktop")
            return web.Response(text=html.replace("</html>", '<script src="/control.js"></script></html>'),
                                content_type="text/html")
        reply = web.StreamResponse(status=upstream.status, headers={
            "Content-Type": upstream.headers.get("Content-Type", "application/octet-stream"),
            "Cache-Control": "no-store"})
        await reply.prepare(request)
        async for block in upstream.content.iter_any():
            await reply.write(block)
        return reply


async def control(request):
    if request.headers.get("Origin") != "http://" + request.host:
        raise web.HTTPForbidden()
    ws = web.WebSocketResponse(max_msg_size=512, heartbeat=15)
    await ws.prepare(request)
    auth = await ws.receive(timeout=5)
    if not isinstance(auth.data, str) or not hmac.compare_digest(auth.data.encode(), control_key.encode()):
        await ws.close(code=1008)
        return ws
    if owner.locked():
        await ws.close(code=1013)
        return ws
    async with owner:
        d = display.Display()
        root = d.screen().root
        held = set()
        try:
            await ws.send_str("ready")
            async for msg in ws:
                if msg.type != web.WSMsgType.TEXT:
                    break
                parts = msg.data.split()
                if len(parts) == 3 and parts[0] == "move":
                    x, y = map(int, parts[1:])
                    if 0 <= x < d.screen().width_in_pixels and 0 <= y < d.screen().height_in_pixels:
                        xtest.fake_input(d, X.MotionNotify, root=root, x=x, y=y)
                elif len(parts) == 3 and parts[0] in ("key", "button") and parts[2] in ("0", "1"):
                    keyboard = parts[0] == "key"
                    code = d.keysym_to_keycode(XK.string_to_keysym(parts[1])) if keyboard else int(parts[1])
                    if not (8 <= code <= 255 if keyboard else 1 <= code <= 7):
                        continue
                    press, release = (X.KeyPress, X.KeyRelease) if keyboard else (X.ButtonPress, X.ButtonRelease)
                    down = parts[2] == "1"
                    xtest.fake_input(d, press if down else release, code)
                    (held.add if down else held.discard)((release, code))
                elif parts == ["release"]:
                    for kind, code in held:
                        xtest.fake_input(d, kind, code)
                    held.clear()
                d.flush()
        finally:
            for kind, code in held:
                xtest.fake_input(d, kind, code)
            d.sync()
            d.close()
    return ws


async def client(app):
    async with ClientSession(auto_decompress=False) as session:
        app["client"] = session
        yield


async def script(request):
    return web.FileResponse(Path(__file__).with_name("control.js"))


app = web.Application()
app.cleanup_ctx.append(client)
app.router.add_get("/control.js", script)
app.router.add_get("/control", control)
app.router.add_get("/{path:.*}", video)
web.run_app(app, host="127.0.0.1", port=6912)
