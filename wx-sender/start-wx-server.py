"""一键启动微信推送服务（供 lol-stats 的设置页连接）。

用法：
  python start-wx-server.py
  python start-wx-server.py --port 8756
"""
import argparse
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def main() -> None:
    parser = argparse.ArgumentParser(description="启动微信推送服务")
    parser.add_argument("--port", type=int, default=8756)
    parser.add_argument("--config", default=str(HERE / "config.json"))
    args = parser.parse_args()

    python = HERE / ".venv" / "Scripts" / "python.exe"
    if not python.is_file():
        python = Path(sys.executable)

    cmd = [
        str(python),
        str(HERE / "wx_server.py"),
        "--config", args.config,
        "--port", str(args.port),
    ]
    print(f"启动: {' '.join(cmd)}")
    print(f"服务地址: http://127.0.0.1:{args.port}")
    print("在 lol-stats 设置 → 微信推送里填这个地址，然后点「检测服务」")
    print("按 Ctrl+C 停止\n")
    try:
        subprocess.run(cmd)
    except KeyboardInterrupt:
        print("\n已停止")


if __name__ == "__main__":
    main()
