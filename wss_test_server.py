#!/usr/bin/env python3
import asyncio
import json
import time
from datetime import datetime, timezone, timedelta

try:
    import websockets
    from websockets import WebSocketServerProtocol
except ImportError:
    print("缺少 websockets 库，请运行: pip install websockets")
    exit(1)

HOST = "0.0.0.0"
WS_PORT = 9443
WS_PATH = "/all_eew"

clients: dict = {}
event_count = 0
report_num_counter = 0
server_start = datetime.now()


def make_event(
    event_type: str,
    magnitude: float,
    latitude: float,
    longitude: float,
    depth_km: float,
    hypocenter: str,
) -> dict:
    global report_num_counter
    report_num_counter += 1
    now = datetime.now(timezone(timedelta(hours=8)))
    event_id = now.strftime("%Y%m%d%H%M%S") + f"{report_num_counter:02d}"

    # 将日本震度映射为数值
    if magnitude >= 7.0:
        max_int_str = "6弱"
        max_int_value = 6.0  # 或根据实际需求映射为6.5等
    elif magnitude >= 6.0:
        max_int_str = "5弱"
        max_int_value = 5.0
    elif magnitude >= 5.0:
        max_int_str = "4"
        max_int_value = 4.0
    elif magnitude >= 4.0:
        max_int_str = "3"
        max_int_value = 3.0
    else:
        max_int_str = "1"
        max_int_value = 1.0

    return {
        "type": event_type,
        "EventID": event_id,
        "ReportNum": report_num_counter,
        "OriginTime": now.strftime("%Y-%m-%d %H:%M:%S"),
        "HypoCenter": hypocenter,
        "Latitude": latitude,
        "Longitude": longitude,
        "Magnitude": magnitude,
        "Depth": depth_km,
        "MaxIntensity": max_int_value,  # 改为数值类型
        "MaxIntensityStr": max_int_str,  # 保留原始震度字符串（可选）
        "Serial": str(report_num_counter),
    }


async def broadcast(data: dict) -> int:
    payload = json.dumps(data, ensure_ascii=False)
    dead: list[WebSocketServerProtocol] = []
    for ws in clients:
        try:
            await ws.send(payload)
        except websockets.ConnectionClosed:
            dead.append(ws)
    for ws in dead:
        clients.pop(ws, None)
    return len(clients)


async def handler(ws: WebSocketServerProtocol):
    peer = ws.remote_address
    addr = f"{peer[0]}:{peer[1]}"
    clients[ws] = addr
    print(f"[+] 客户端已连接 ({addr})  [#clients={len(clients)}]")
    try:
        async for msg in ws:
            pass
    except websockets.ConnectionClosed:
        pass
    finally:
        clients.pop(ws, None)
        print(f"[-] 客户端已断开 ({addr})  [#clients={len(clients)}]")


async def cmd_send(args: list[str]):
    if len(args) < 6:
        print("用法: send <type> <mag> <lat> <lon> <depth> <hypocenter...>")
        return
    event_type = args[0]
    try:
        mag = float(args[1])
        lat = float(args[2])
        lon = float(args[3])
        depth = float(args[4])
    except ValueError:
        print("震级/纬度/经度/深度 必须为数值")
        return
    hypocenter = " ".join(args[5:]) if args[5:] else "未知"

    if not clients:
        print("\n  ⚠ 没有已连接的客户端\n")
        return

    data = make_event(event_type, mag, lat, lon, depth, hypocenter)
    global event_count
    event_count += 1
    n = await broadcast(data)
    t = data["type"]
    print(f"\n  >> [{t}] M{mag} {hypocenter} ({lat},{lon}) depth={depth}km")
    print(f"  >> 已发送到 {n} 个客户端  [EventID={data['EventID']}]\n")


async def cmd_json(args: list[str]):
    raw = " ".join(args)
    if not raw:
        print("用法: json <json_string>")
        return
    try:
        data = json.loads(raw)
    except json.JSONDecodeError as e:
        print(f"JSON 解析错误: {e}")
        return
    if not clients:
        print("\n  ⚠ 没有已连接的客户端\n")
        return
    global event_count
    event_count += 1
    n = await broadcast(data)
    print(f"\n  >> 自定义事件已发送到 {n} 个客户端\n")


async def cmd_clients():
    if not clients:
        print("  没有已连接的客户端")
        return
    print(f"  已连接客户端 ({len(clients)}):")
    for i, addr in enumerate(clients.values(), 1):
        print(f"    {i}. {addr}")


async def cmd_status():
    elapsed = datetime.now() - server_start
    secs = int(elapsed.total_seconds())
    print(f"  运行时间: {secs // 3600:02d}:{(secs % 3600) // 60:02d}:{secs % 60:02d}")
    print(f"  已发送事件: {event_count}")
    print(f"  当前客户端: {len(clients)}")


async def cmd_help():
    print("""\
命令:
  send <type> <mag> <lat> <lon> <depth> <hypocenter...>
        发送地震事件 (自动生成 EventID/OriginTime/ReportNum)
        例: send cenc_eew 6.5 31.9 102.2 10 四川阿坝

  json <json_string>
        发送原始自定义 JSON 事件
        例: json {"type":"cenc_eew","Magnitude":6.0}

  clients   查看已连接的 WebSocket 客户端
  status    查看服务器统计
  help      显示本帮助
  quit      退出服务器""")


async def repl():
    print("EEW 测试服务器 - 交互式控制台")
    print(f"WS: ws://{HOST}:{WS_PORT}{WS_PATH}")
    print(f"输入 help 查看命令\n")
    loop = asyncio.get_running_loop()

    while True:
        line = await loop.run_in_executor(None, input, "> ")
        line = line.strip()
        if not line:
            continue

        parts = line.split()
        cmd = parts[0].lower()

        if cmd in ("quit", "exit", "q"):
            print("正在关闭服务器...")
            break
        elif cmd == "send":
            await cmd_send(parts[1:])
        elif cmd == "json":
            await cmd_json(parts[1:])
        elif cmd == "clients":
            await cmd_clients()
        elif cmd == "status":
            await cmd_status()
        elif cmd in ("help", "?"):
            await cmd_help()
        else:
            print(f"未知命令: {cmd}   (输入 help 查看可用命令)")


async def main():
    async with websockets.serve(
        handler,
        HOST,
        WS_PORT,
        ping_interval=30,
        ping_timeout=10,
    ):
        print(f"WebSocket 服务器启动: ws://{HOST}:{WS_PORT}{WS_PATH}")
        await repl()

    for ws in list(clients.keys()):
        await ws.close()
    clients.clear()
    print("服务器已关闭")


if __name__ == "__main__":
    asyncio.run(main())
