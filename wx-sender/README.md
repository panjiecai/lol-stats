# 微信推送服务

给 lol-stats 提供「对局结束后把战绩图直接发到微信群」的本地服务。

## 它能做什么

对局结束后自动生成战绩卡片并发到指定微信群，全程不占剪贴板、不抢前台窗口，可以边玩边发。

```
lol-stats（轮询 LCU → 渲染卡片 → 截图）
        ↓  HTTP POST（base64 PNG）
wx_server.py（常驻，127.0.0.1:8756）
        ↓  进程内调用原生桥
微信进程内发送函数
        ↓
微信群
```

## 它做不到什么

**这不是官方接口。** 三件事决定了它的适用范围：

1. **微信版本必须命中适配表**：4.1.15.13 / 4.1.15.56 / 4.2.0.11 / 4.2.0.12 / 4.2.0.13 / 4.2.0.52 / 4.2.0.53
2. **原生桥必须在本机编译**：`dsh-wechat.dll` 与微信版本一一对应，不能跨机复用
3. **只能发给白名单里的聊天**：必须显式配置，否则服务直接拒绝

## ⚠️ 风险声明

实现方式是**解密读取微信本地数据库 + 向微信进程注入 DLL 调用内部函数**。

即使做到了不开窗口、不碰剪贴板、不模拟按键，仍然：

- 可能被判定为异常使用，导致账号功能受限或封禁
- 微信更新即失效，需等适配表更新后重新编译
- 无任何官方保障

**建议只用自用号/小号，不要用在主号上。**

## 安装

### 1. 确认前置

```bash
# 微信版本必须在适配表里
powershell -Command "(Get-Process Weixin | Select-Object -First 1).Modules | ? {$_.ModuleName -eq 'Weixin.dll'} | Select-Object FileName"

# Python 3.10+
python --version

# VS C++ 生成工具（编译原生桥必需）
"C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe" -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
```

### 2. 装依赖

```bash
cd wx-sender
python -m venv .venv
.venv\Scripts\python.exe -m pip install -r requirements.txt
```

### 3. 配置

```bash
cp config.example.json config.json
```

编辑 `config.json`：

| 字段 | 说明 |
|---|---|
| `account` | 微信数据目录名，如 `wxid_xxxxxxxx_a1b2c3`（含后缀） |
| `selfId` | 你的 wxid，如 `wxid_xxxxxxxx`（不含后缀） |
| `dbDir` | 微信数据根目录，如 `D:/Documents/weixin/xwechat_files` |
| `mediaRoots` | 允许发送的图片目录（绝对路径） |
| `chats` | 白名单，可从「文件传输助手」开始 |

微信数据目录在「微信 → 设置 → 文件管理」里能看到。

### 4. 编译原生桥（不能跳过）

```bash
powershell -ExecutionPolicy Bypass -File .\engine\build.ps1 -OutDir "<此目录绝对路径>\engine"
```

产物：

```
engine/
├── current.json          指向本次编译的 DLL
├── profiles.json         微信版本适配表
└── <hash>/dsh-wechat.dll 原生桥
```

### 5. 验证

```bash
# 启动服务
.venv\Scripts\python.exe wx_server.py --port 8756
```

```bash
# 另一个窗口：健康检查
curl http://127.0.0.1:8756/health
# → {"ok": true, "chats": [...]}

# 列出本机所有可发聊天（读会话库，约 10 秒）
curl http://127.0.0.1:8756/available
```

能列出群和联系人 → 账号绑定与数据库读取都正常。

## HTTP 接口

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/health` | 服务状态 + 当前白名单 |
| GET | `/config` | 账号与白名单详情 |
| GET | `/available` | 列出本机所有可发聊天（标注是否已在白名单） |
| POST | `/config/chats` | 保存白名单 `{"chats":[{"id","name"}]}` |
| POST | `/push` | 发送 `{"chat","image"(base64)|"imagePath","text"}` |

服务只监听 `127.0.0.1`，不对外开放。

## 在 lol-stats 里配置

监控面板 → 微信推送：

1. 填服务地址 `http://127.0.0.1:8756`
2. 点「连接服务」
3. 左侧列出本机所有聊天，把要发的加进右侧白名单
4. 点「保存」
5. **勾选**要发送的目标（这一步才决定真的发）

白名单和勾选是两道独立的关卡：白名单决定服务**允许**发往哪些群，勾选决定**这次**发给谁。

## 打包成 exe

```bash
.venv\Scripts\python.exe build-exe.py
```

产物在 `dist/`：

```
wx-push-server.exe        约 116MB，含 Python 运行时
config.json 等            运行时依赖
engine/                   原生桥源码 + 适配表
```

**exe 仍不含可移植的原生桥 DLL**：拷到新机器后要在本机重跑 `engine\build.ps1`。

## 已知限制

| 限制 | 说明 |
|---|---|
| **无投递回执** | 微信 4.x 持续写入时消息库拿不到稳定快照，只确认原生调用成功，不确认对方收到 |
| **服务需常驻** | 关掉就停推；没有开机自启 |
| **微信重登要重启服务** | 进程 PID 变了，旧句柄失效 |
| **exe 跨机仍需重编译** | 原生桥与微信版本绑定 |

## 许可

本目录的 Python 代码沿用仓库根目录的 MIT 许可。

**例外**：`engine/` 下的原生桥（`bridge.cpp`、`profiles.json` 等）来自
[Prometheus-first/helper](https://github.com/Prometheus-first/helper)，
适用其 **非商业使用许可协议**（见 `engine/LICENSE`），**不受 MIT 覆盖**。
`engine/NOTICE.md` 记录了具体的来源范围。

分发或商用前请先阅读 `engine/LICENSE`。

## 换机部署

详细步骤见 [换机校对清单.md](./换机校对清单.md)。
