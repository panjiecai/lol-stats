"""纯原生后台发送 CLI：不读消息库、不建快照、不开窗口、不动剪贴板。

直接复用 sender.NativeSender —— 向已登录的微信进程加载 DLL 并调用内部发送函数。
微信 4.x 持续写入时消息库永远拿不到稳定快照，但发送本身不需要读库。

发送逻辑在 wx_send_core，服务进程与 CLI 共用同一份实现。
"""
import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from wx_send_core import sendNow  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description="后台发送图片/文字到微信聊天")
    parser.add_argument("--config", required=True)
    parser.add_argument("--chat", required=True)
    parser.add_argument("--text", action="append", default=[])
    parser.add_argument("--image", action="append", default=[])
    parser.add_argument("--allow-send", action="store_true")
    args = parser.parse_args()

    if not args.allow_send:
        raise SystemExit("发送需要显式提供 --allow-send")

    result = sendNow(Path(args.config), args.chat, args.text, args.image)
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
