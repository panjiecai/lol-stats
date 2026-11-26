"""本地微信推送服务：给你的 lol-stats 提供「截图后直接发微信群」的接口。

设计要点：
  - 只监听 127.0.0.1，不对外网暴露
  - 保持常驻，避免每次截图都重新加载原生桥
  - 真正发送走子进程 send_native.py，崩溃或卡住都不影响服务本身
  - 不读消息库（微信 4.x 持续写入时读不到稳定快照），只做单向投递

接口：
  GET  /health                  → {"ok":true,"chats":[...]}
  POST /push                     body: {"chat":"xxx@chatroom", "image":"<base64>", "text":"可选"}
                               或   {"chat":"...", "imagePath":"绝对路径", "text":"..."}
"""
import argparse
import base64
import binascii
import json
import re
import sys
import threading
import time
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


def appDir() -> Path:
    """运行时文件（config.json / engine / *.py）所在目录。

    打包成 exe 后 __file__ 指向 PyInstaller 的临时解压目录，
    而 config.json、原生桥 DLL 要留在 exe 旁边，所以按是否冻结区分。
    """
    if getattr(sys, "frozen", False):
        return Path(sys.executable).resolve().parent
    return Path(__file__).resolve().parent


HERE = appDir()
SCRIPT_DIR = Path(__file__).resolve().parent
# 冻结后同目录的 *.py（adapter/native/sender…）仍需可导入
if str(HERE) not in sys.path:
    sys.path.insert(0, str(HERE))
MAX_BODY = 32 * 1024 * 1024  # 32MB，够 2 倍像素比的战绩图
CHAT_RE = re.compile(r"^[A-Za-z0-9_.@-]{1,127}$")


def log(message: str) -> None:
    print(f"[{datetime.now():%H:%M:%S}] {message}", flush=True)


class Bridge:
    def __init__(self, configPath: Path):
        self.configPath = configPath
        self.lock = threading.Lock()
        self.queue: list = []
        self.worker = threading.Thread(target=self._run, daemon=True)
        self.worker.start()

    def config(self) -> dict:
        return json.loads(self.configPath.read_text(encoding="utf-8-sig"))

    def chats(self) -> list:
        return self.config().get("chats", [])

    def mediaRoot(self) -> Path:
        roots = self.config().get("mediaRoots") or []
        if not roots:
            raise ValueError("config.json 缺少 mediaRoots")
        root = Path(roots[0]).resolve()
        root.mkdir(parents=True, exist_ok=True)
        return root

    def submit(self, job: dict) -> dict:
        with self.lock:
            self.queue.append(job)
        return {"ok": True, "queued": len(self.queue)}

    def describe(self) -> dict:
        config = self.config()
        return {
            "account": config.get("account", ""),
            "chats": config.get("chats", []),
            "mediaRoots": config.get("mediaRoots", []),
        }

    def saveChats(self, chats: list) -> list:
        """保存白名单。只收 {id,name,trigger?}，其余字段一律丢弃，避免写坏 config.json。"""
        cleaned = []
        seen = set()
        for item in chats:
            if not isinstance(item, dict):
                raise ValueError("每个聊天必须是对象")
            chatId = str(item.get("id") or "").strip()
            name = str(item.get("name") or "").strip()
            if not CHAT_RE.match(chatId):
                raise ValueError(f"非法 chat id：{chatId!r}")
            if chatId in seen:
                continue
            seen.add(chatId)
            entry = {"id": chatId, "name": name or chatId}
            if chatId.endswith("@chatroom"):
                entry["trigger"] = "mention"
            cleaned.append(entry)

        if not cleaned:
            raise ValueError("至少要保留一个聊天")

        with self.lock:
            config = self.config()
            config["chats"] = cleaned
            raw = self.configPath.read_text(encoding="utf-8-sig")
            tmp = self.configPath.with_suffix(".tmp")
            tmp.write_text(json.dumps(config, ensure_ascii=False, indent=2), encoding="utf-8")
            tmp.replace(self.configPath)
        log(f"白名单已更新：{', '.join(c['name'] for c in cleaned)}")
        return cleaned

    def availableChats(self) -> list:
        """列出本机微信里所有可发的聊天，供配置页挑选。

        只读会话列表（session 库），不碰消息内容；拿不到就返回空，
        配置页会退化成只能手填 id。
        """
        try:
            from adapter import exclusiveRun  # noqa: WPS433
            from native import connect  # noqa: WPS433

            config = self.config()
            stateDir = self.configPath.parent / config["account"]
            stateDir.mkdir(parents=True, exist_ok=True)
            config = {**config, "engineDir": str(self.configPath.parent / "engine")}

            with exclusiveRun(stateDir):
                database, send = connect(config, stateDir)
                try:
                    rows = database.get_sessions(limit=200)
                finally:
                    if send is not None:
                        send.close()

            whitelisted = {c["id"] for c in config.get("chats", [])}
            result = []
            for row in rows:
                chatId = row.get("username") or ""
                if not chatId or chatId.startswith("@placeholder"):
                    continue
                try:
                    contacts = database.search_contact(chatId)
                    contact = next((c for c in contacts if c.get("username") == chatId), {})
                except Exception:
                    contact = {}
                name = (
                    contact.get("remark")
                    or contact.get("nick_name")
                    or row.get("str_content")
                    or chatId
                )
                result.append({
                    "id": chatId,
                    "name": str(name)[:60],
                    "isGroup": chatId.endswith("@chatroom"),
                    "whitelisted": chatId in whitelisted,
                })
            return result
        except Exception as error:
            log(f"列举聊天失败（配置页可手填）：{error}")
            return []

    def _run(self) -> None:
        # 串行发送：微信原生调用不能并发，且要保持顺序
        while True:
            job = None
            with self.lock:
                if self.queue:
                    job = self.queue.pop(0)
            if job is None:
                time.sleep(0.2)
                continue
            try:
                self._deliver(job)
            except Exception as error:
                log(f"发送失败 chat={job.get('chat')}: {error}")

    def _deliver(self, job: dict) -> None:
        chat = job["chat"]
        # 进程内直接调，不再 spawn Python：打包成 exe 后没有解释器可 spawn
        from wx_send_core import sendNow

        texts = [job["text"]] if job.get("text") else []
        images = [job["imagePath"]] if job.get("imagePath") else []
        sendNow(self.configPath, chat, texts, images)
        log(f"已发送 → {chat}  {job.get('imagePath') or job.get('text', '')[:40]}")


def buildHandler(bridge: Bridge):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format, *args):  # 静音默认访问日志
            pass

        def _send(self, status: int, payload: dict) -> None:
            body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
            self.send_response(status)
            self.send_header("Content-Type", "application/json; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Access-Control-Allow-Headers", "content-type")
            self.send_header("Access-Control-Allow-Methods", "GET,POST,OPTIONS")
            self.end_headers()
            self.wfile.write(body)

        def do_OPTIONS(self):
            self._send(204, {})

        def do_GET(self):
            if self.path.startswith("/health"):
                try:
                    self._send(200, {"ok": True, "chats": bridge.chats()})
                except Exception as error:
                    self._send(500, {"ok": False, "error": str(error)})
                return
            if self.path.startswith("/config"):
                try:
                    self._send(200, {"ok": True, **bridge.describe()})
                except Exception as error:
                    self._send(500, {"ok": False, "error": str(error)})
                return
            if self.path.startswith("/available"):
                try:
                    self._send(200, {"ok": True, "candidates": bridge.availableChats()})
                except Exception as error:
                    self._send(500, {"ok": False, "error": str(error)})
                return
            self._send(404, {"ok": False, "error": "unknown path"})

        def do_POST(self):
            if self.path.startswith("/config/chats"):
                self._handleSaveChats()
                return
            if not self.path.startswith("/push"):
                self._send(404, {"ok": False, "error": "unknown path"})
                return

            length = int(self.headers.get("Content-Length") or 0)
            if length <= 0 or length > MAX_BODY:
                self._send(413, {"ok": False, "error": f"请求体需 1~{MAX_BODY} 字节"})
                return

            try:
                payload = json.loads(self.rfile.read(length))
            except ValueError:
                self._send(400, {"ok": False, "error": "body 不是合法 JSON"})
                return

            chat = (payload.get("chat") or "").strip()
            if not CHAT_RE.match(chat):
                self._send(400, {"ok": False, "error": "chat 为空或含非法字符"})
                return

            try:
                allowed = {c["id"] for c in bridge.chats()}
            except Exception as error:
                self._send(500, {"ok": False, "error": str(error)})
                return
            if chat not in allowed:
                self._send(403, {"ok": False, "error": f"{chat} 不在 config.json 的 chats 里"})
                return

            text = payload.get("text") or ""
            if len(text.encode("utf-8")) > 4000:
                self._send(400, {"ok": False, "error": "text 超过 4000 字节"})
                return

            imagePath = None
            if payload.get("image"):
                try:
                    raw = base64.b64decode(payload["image"], validate=True)
                except (binascii.Error, TypeError):
                    self._send(400, {"ok": False, "error": "image 不是合法 base64"})
                    return
                if not raw.startswith(b"\x89PNG"):
                    self._send(400, {"ok": False, "error": "图片不是 PNG"})
                    return
                try:
                    root = bridge.mediaRoot()
                except Exception as error:
                    self._send(500, {"ok": False, "error": str(error)})
                    return
                stamp = datetime.now().strftime("%Y%m%d-%H%M%S-%f")
                imagePath = root / f"push-{stamp}.png"
                imagePath.write_bytes(raw)
            elif payload.get("imagePath"):
                candidate = Path(payload["imagePath"]).resolve()
                try:
                    root = bridge.mediaRoot()
                except Exception as error:
                    self._send(500, {"ok": False, "error": str(error)})
                    return
                if not candidate.is_relative_to(root):
                    self._send(403, {"ok": False, "error": "imagePath 必须在 mediaRoots 内"})
                    return
                if not candidate.is_file():
                    self._send(404, {"ok": False, "error": "imagePath 不存在"})
                    return
                imagePath = candidate

            if imagePath is None and not text:
                self._send(400, {"ok": False, "error": "至少需要 image、imagePath 或 text"})
                return

            self._send(200, bridge.submit({
                "chat": chat,
                "text": text,
                "imagePath": str(imagePath) if imagePath else None,
            }))

        def _handleSaveChats(self):
            length = int(self.headers.get("Content-Length") or 0)
            if length <= 0 or length > 64 * 1024:
                self._send(413, {"ok": False, "error": "请求体过大或为空"})
                return
            try:
                payload = json.loads(self.rfile.read(length))
            except ValueError:
                self._send(400, {"ok": False, "error": "body 不是合法 JSON"})
                return

            chats = payload.get("chats")
            if not isinstance(chats, list):
                self._send(400, {"ok": False, "error": "chats 必须是数组"})
                return

            try:
                saved = bridge.saveChats(chats)
            except ValueError as error:
                self._send(400, {"ok": False, "error": str(error)})
                return
            except Exception as error:
                self._send(500, {"ok": False, "error": str(error)})
                return

            self._send(200, {"ok": True, "chats": saved})

    return Handler


def main() -> None:
    parser = argparse.ArgumentParser(description="微信群聊推送本地服务")
    parser.add_argument("--config", default=str(HERE / "config.json"))
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8756)
    args = parser.parse_args()

    configPath = Path(args.config).resolve()
    if not configPath.is_file():
        raise SystemExit(f"找不到配置 {configPath}")

    bridge = Bridge(configPath)
    server = ThreadingHTTPServer((args.host, args.port), buildHandler(bridge))
    log(f"微信推送服务已启动 http://{args.host}:{args.port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        log("收到中断，退出")


if __name__ == "__main__":
    main()
