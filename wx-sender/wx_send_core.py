"""发送核心：给 CLI 和常驻服务共用的一套逻辑。

单独拆出来是为了打包成 exe 后能直接调用，不必再 spawn 一个 Python 进程。
"""
import json
from pathlib import Path

from adapter import exclusiveRun, validateConfig
from binding import findAccountProcess, checkAccountProcess
from outgoing import prepareMessages
from sender import NativeSender


def sendNow(configPath: Path, chatId: str, texts=None, images=None) -> dict:
    """同步发送一批内容到指定聊天。调用方负责串行化。"""
    texts = list(texts or [])
    images = list(images or [])
    if not texts and not images:
        raise ValueError("至少提供 --text 或 --image")

    configPath = Path(configPath).resolve()
    config = json.loads(configPath.read_text(encoding="utf-8-sig"))
    validateConfig(config)

    chat = next((c for c in config["chats"] if c["id"] == chatId), None)
    if chat is None:
        raise ValueError(f"聊天 {chatId} 不在 config.json 的 chats 中")

    stateDir = configPath.parent / config["account"]
    stateDir.mkdir(parents=True, exist_ok=True)
    config["engineDir"] = str(configPath.parent / "engine")

    items = [dict(type="text", text=t) for t in texts]
    items += [dict(type="image", path=str(Path(p).resolve())) for p in images]
    prepared = prepareMessages(items, config, stateDir)

    process = findAccountProcess(config)
    started = process.create_time()
    config = {**config, "processId": process.pid}

    def checkProcess():
        checkAccountProcess(process, config, started)

    with exclusiveRun(stateDir):
        sender = NativeSender(config, checkProcess)
        try:
            for item in prepared:
                kind = {"text": 1, "image": 2, "sticker": 3}[item["type"]]
                value = item["text"] if kind == 1 else item["path"]
                sender.send(chat["id"], kind, value)
        finally:
            sender.close()

    return {"ok": True, "chat": chat["id"], "sent": len(prepared)}
