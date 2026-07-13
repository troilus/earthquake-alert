#!/usr/bin/env python3
"""
FAN Studio WebSocket Monitor — 中国气象局气象预警
监听 wss://ws.fanstudio.tech/all，仅提取 weatheralarm 数据推送到 Bark
"""

import asyncio
import json
import os
import ssl
from datetime import datetime

import requests
import websockets

_SSL_CTX = ssl._create_unverified_context()

# ── 配置 ────────────────────────────────────────────────
BARK_URL = "https://api.day.app"
BARK_DEVICE_KEY = ""
BARK_GROUP = "气象预警"
BARK_LEVEL = "timeSensitive"
BARK_SOUND = "alarm"
BARK_VOLUME = 10
BARK_CALL = "1"
WEBSOCKET_URL = "wss://ws.fanstudio.tech/all"
RECONNECT_MIN = 1
RECONNECT_MAX = 30
MAX_BODY_CHARS = 4000


def load_config():
    g = globals()
    for key in ("BARK_URL", "BARK_DEVICE_KEY", "BARK_GROUP", "BARK_LEVEL", "BARK_SOUND", "BARK_CALL"):
        g[key] = os.getenv(key, g[key])
    g["BARK_VOLUME"] = int(os.getenv("BARK_VOLUME", str(BARK_VOLUME)))
    g["WEBSOCKET_URL"] = os.getenv("FANSTUDIO_WEBSOCKET_URL", WEBSOCKET_URL)
    g["RECONNECT_MIN"] = int(os.getenv("RECONNECT_MIN_SECONDS", str(RECONNECT_MIN)))
    g["RECONNECT_MAX"] = int(os.getenv("RECONNECT_MAX_SECONDS", str(RECONNECT_MAX)))


def log(msg: str):
    print(f"[{datetime.now():%H:%M:%S}] {msg}", flush=True)


def send_bark(title: str, body: str):
    if not BARK_DEVICE_KEY:
        return
    url = f"{BARK_URL.rstrip('/')}/push"
    payload = {
        "device_key": BARK_DEVICE_KEY,
        "title": title,
        "body": body[:MAX_BODY_CHARS],
        "group": BARK_GROUP,
        "level": BARK_LEVEL,
    }
    if BARK_LEVEL != "passive":
        payload["volume"] = BARK_VOLUME
        if BARK_CALL == "1":
            payload["call"] = "1"
        if BARK_SOUND:
            payload["sound"] = BARK_SOUND
    try:
        resp = requests.post(url, json=payload, timeout=5)
        if resp.status_code == 200:
            log(f"  ✓ Bark 推送成功")
        else:
            log(f"  ✗ Bark ({resp.status_code}): {resp.text[:120]}")
    except requests.RequestException as e:
        log(f"  ✗ Bark: {e}")


def format_weather_alarm(data: dict) -> tuple[str, str]:
    title = data.get("headline") or data.get("title") or "气象预警"
    desc = data.get("description", "")
    effective = data.get("effective", "")
    lat = data.get("latitude")
    lon = data.get("longitude")
    alarm_type = data.get("type", "")

    lines = []
    if effective:
        lines.append(f"发布时间: {effective}")
    if desc:
        lines.append("")
        lines.append(desc)
    if lat and lon:
        lines.append("")
        lines.append(f"中心位置: {lat:.4f}, {lon:.4f}")
    if alarm_type:
        lines.append(f"预警类型: {alarm_type}")

    return title, "\n".join(lines)


async def monitor():
    load_config()
    if not BARK_DEVICE_KEY:
        log(" BARK_DEVICE_KEY 未设置，仅打印到控制台")
    else:
        log(f" Bark: {BARK_URL}")
    log(f" 监听 {WEBSOCKET_URL}（仅 weatheralarm）")

    delay = RECONNECT_MIN
    last_md5 = ""

    while True:
        try:
            log(" 连接中...")
            async with websockets.connect(
                WEBSOCKET_URL,
                max_size=8 * 1024 * 1024,
                ping_interval=90,
                ping_timeout=10,
                ssl=_SSL_CTX,
            ) as ws:
                log(" 已连接")
                delay = RECONNECT_MIN

                async for raw in ws:
                    try:
                        msg = json.loads(raw)
                    except json.JSONDecodeError:
                        continue

                    mtype = msg.get("type")

                    if mtype == "heartbeat":
                        await ws.send("ping")
                        continue

                    # ── 初始快照 / 查询响应 ──
                    if mtype in ("initial_all", "query_response"):
                        snap = msg.get("weatheralarm")
                        if not isinstance(snap, dict):
                            continue
                        data = snap.get("Data")
                        if not isinstance(data, dict):
                            continue
                        md5 = snap.get("md5", "")
                        if md5:
                            last_md5 = md5
                        title, body = format_weather_alarm(data)
                        log(f"  快照: {title}")
                        send_bark(title, body)
                        continue

                    # ── 增量更新 ──
                    if mtype == "update" and msg.get("source") == "weatheralarm":
                        data = msg.get("Data")
                        if not isinstance(data, dict):
                            continue
                        md5 = msg.get("md5", "")
                        if md5 and md5 == last_md5:
                            continue
                        if md5:
                            last_md5 = md5
                        title, body = format_weather_alarm(data)
                        log(f"  更新: {title}")
                        send_bark(title, body)
                        continue

        except websockets.ConnectionClosed as e:
            log(f" 断开 (code={e.code})")
        except asyncio.CancelledError:
            raise
        except Exception as e:
            log(f" 异常: {e}")

        log(f" {delay}s 后重连...")
        await asyncio.sleep(delay)
        delay = min(delay * 2, RECONNECT_MAX)


def main():
    try:
        from dotenv import load_dotenv
        load_dotenv()
    except ImportError:
        pass
    load_config()
    try:
        asyncio.run(monitor())
    except KeyboardInterrupt:
        log(" 已停止")


if __name__ == "__main__":
    main()
