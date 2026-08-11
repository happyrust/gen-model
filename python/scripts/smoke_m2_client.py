# -*- coding: utf-8 -*-
"""M2 HTTP 客户端验收：REST 只读端点 + WebSocket 握手（不触发 execute，避免真实改数据）。"""

import json
import sys
from pathlib import Path

sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import websocket

from aios_client import AiosClient


def main() -> None:
    client = AiosClient()

    health = client.health()
    print(f"/health: project={health['project']} sync_live={health['sync_live']} "
          f"queue_paused={health['queue_paused']} version={health['version']}")

    dbnums = client.dbnums()
    print(f"/dbnums: {len(dbnums['dbnums'])} 行，blocked="
          f"{[r['dbnum'] for r in dbnums['dbnums'] if r.get('blocked')]}")

    queue = client.queue()
    print(f"/queue: paused={queue['paused']} rows={len(queue['rows'])}")

    tasks = client.tasks(limit=5)
    print(f"/tasks: {len(tasks.get('tasks', tasks if isinstance(tasks, list) else []))} 行")

    units = client.pending_units()
    print(f"/update/pending-units: {len(units['units'])} 行")

    preview = client.update_preview()
    print(f"/update/preview: up_to_date={preview['up_to_date']} dbnums={len(preview['dbnums'])}")

    # WebSocket：订阅 + 心跳往返
    ws_url = client.base.replace("http://", "ws://") + "/api/v1/ws"
    conn = websocket.create_connection(ws_url, timeout=10)
    conn.send(json.dumps({"type": "subscribe", "topics": ["tasks"]}))
    conn.send(json.dumps({"type": "ping"}))
    reply = json.loads(conn.recv())
    conn.close()
    print(f"/ws: 订阅成功，ping -> {reply['type']}")
    assert reply["type"] == "pong"

    print("\nM2 HTTP 客户端验收通过。")


if __name__ == "__main__":
    main()
