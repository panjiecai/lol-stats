"""打包微信推送服务为独立 exe。

用法：
  .venv\\Scripts\\python.exe build-exe.py

产物：
  dist/wx-push-server.exe            单文件服务（含 Python 运行时与全部依赖）
  dist/wx-push-server/               onedir 备选（启动更快）

注意：原生桥 DLL（dsh-wechat.dll）不打包进 exe —— 它依赖具体的微信版本，
必须在目标机上用 wechat.ps1 Build 编译。exe 只负责「找到它并调用」。
"""
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIST = HERE / "dist"
BUILD = HERE / "build"

# 服务运行时会读取这些同目录文件，必须一起分发
RUNTIME_FILES = [
    "config.json",
    "send_native.py",
    "wx_send_core.py",
    "adapter.py",
    "binding.py",
    "groups.py",
    "incoming.py",
    "messages.py",
    "native.py",
    "outgoing.py",
    "presence.py",
    "quotes.py",
    "reading.py",
    "sender.py",
    "snapshot.py",
    "stickers.py",
]

# PyInstaller 看不到的动态导入：native.py 里 import wechatauto.*
HIDDEN_IMPORTS = [
    "wechatauto",
    "wechatauto.db",
    "wechatauto.param",
    "PIL",
    "PIL.Image",
    "psutil",
]


def main() -> None:
    python = HERE / ".venv" / "Scripts" / "python.exe"
    if not python.is_file():
        python = Path(sys.executable)

    for stale in (DIST, BUILD):
        if stale.exists():
            shutil.rmtree(stale)

    cmd = [
        str(python), "-m", "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onefile",
        "--name", "wx-push-server",
        "--distpath", str(DIST),
        "--workpath", str(BUILD),
        "--specpath", str(BUILD),
        # 服务端只用标准库 + wechatauto，不要拖进无关的大包
        "--exclude-module", "numpy",
        "--exclude-module", "pandas",
        "--exclude-module", "matplotlib",
        "--exclude-module", "scipy",
        "--exclude-module", "tkinter",
        "--exclude-module", "PyQt5",
        "--exclude-module", "PySide6",
    ]
    for name in HIDDEN_IMPORTS:
        cmd += ["--hidden-import", name]
    cmd.append(str(HERE / "wx_server.py"))

    print(f"[build] {' '.join(cmd)}\n")
    result = subprocess.run(cmd, cwd=str(HERE))
    if result.returncode != 0:
        raise SystemExit(f"PyInstaller 失败，退出码 {result.returncode}")

    exe = DIST / "wx-push-server.exe"
    if not exe.is_file():
        raise SystemExit("没有生成 exe")

    # 把运行时依赖复制到 exe 旁边
    for name in RUNTIME_FILES:
        src = HERE / name
        if src.is_file():
            shutil.copy2(src, DIST / name)

    engine = HERE / "engine"
    if engine.is_dir():
        shutil.copytree(engine, DIST / "engine", dirs_exist_ok=True)

    size_mb = exe.stat().st_size / 1024 / 1024
    print(f"\n[build] 完成：{exe}  ({size_mb:.1f} MB)")
    print(f"[build] 运行时文件已复制到 {DIST}")
    print("\n使用：双击 wx-push-server.exe，或从 lol-stats 的监控面板点「启动服务」")


if __name__ == "__main__":
    main()
