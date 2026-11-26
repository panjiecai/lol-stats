//! 微信推送服务（wx-sender/wx_server.py）的进程生命周期托管。
//!
//! 背景：wx_server.py 是独立进程，Tauri 只能通过 HTTP 跟它说话。
//! 如果不托管，就会出现「应用关了服务还在跑」「服务挂了应用不知道」。
//!
//! 做法：
//!   1. 启动时先探一次 127.0.0.1:8756/health，已有服务就直接复用（别人起的进程不动它）
//!   2. 没有就自己拉起来，并塞进一个 Windows Job Object（KILL_ON_JOB_CLOSE）
//!      —— 即使应用被任务管理器强杀，最后一个句柄随进程消失，Python 也会被系统连带结束
//!   3. 正常退出时再显式 TerminateJobObject，保证立刻回收而不是等系统清理
//!
//! 注意：探活和拉起都放在后台线程里做，`setup` 不能被它拖住。

use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

/// 与前端 `DEFAULT_WX_PUSH_URL` 保持一致。
const PORT: u16 = 8756;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(300);
/// /health 内部要解密微信会话库，冷启动时可能要 1s 以上，给宽松些。
const READ_TIMEOUT: Duration = Duration::from_millis(3000);
const READY_TIMEOUT: Duration = Duration::from_secs(20);

enum Slot {
    /// 端口上原本就有服务，不是我们拉起的，退出时不能杀。
    External,
    Owned(Service),
}

struct Service {
    child: Child,
    /// Job Object 句柄，0 表示创建失败（此时只能靠 child.kill 兜底）。
    #[cfg(windows)]
    job: isize,
}

static SERVICE: Mutex<Option<Slot>> = Mutex::new(None);
/// 应用已经在退出，别再拉起新进程。
static STOPPING: AtomicBool = AtomicBool::new(false);

fn lock() -> std::sync::MutexGuard<'static, Option<Slot>> {
    SERVICE.lock().unwrap_or_else(|error| error.into_inner())
}

/// 应用启动时调用，立即返回。
pub fn start() {
    thread::spawn(|| start_blocking(true));
}

fn start_blocking(wait_ready_it: bool) {
    if STOPPING.load(Ordering::Acquire) {
        return;
    }

    if lock().is_some() {
        return;
    }

    if probe() {
        eprintln!("[wx-service] {PORT} 已有服务在跑，直接复用（退出时不回收）");
        *lock() = Some(Slot::External);
        return;
    }

    let Some(dir) = resolve_dir() else {
        eprintln!("[wx-service] 未找到 wx-sender 目录，跳过托管");
        return;
    };

    match spawn(&dir) {
        Ok(service) => {
            if STOPPING.load(Ordering::Acquire) {
                eprintln!("[wx-service] 应用正在退出，放弃本次拉起");
                kill(service);
                return;
            }
            eprintln!("[wx-service] 已拉起 {} 下的服务", display_path(&dir));
            *lock() = Some(Slot::Owned(service));
            if wait_ready_it {
                wait_ready();
            }
        }
        Err(error) => eprintln!("[wx-service] 拉起失败：{error}"),
    }
}

/// 应用退出时调用。
pub fn stop() {
    STOPPING.store(true, Ordering::Release);
    let Some(Slot::Owned(service)) = lock().take() else {
        return;
    };
    kill(service);
    eprintln!("[wx-service] 服务已随应用退出");
}

fn kill(mut service: Service) {
    #[cfg(windows)]
    unsafe {
        if service.job != 0 {
            // 1 只是退出码，作用是把 Job 里的所有进程一起终止
            windows_sys::Win32::System::JobObjects::TerminateJobObject(service.job as _, 1);
        }
    }

    let _ = service.child.kill();
    let _ = service.child.wait();

    #[cfg(windows)]
    unsafe {
        if service.job != 0 {
            windows_sys::Win32::Foundation::CloseHandle(service.job as _);
        }
    }
}

/// 当前状态，供前端/排查使用。
pub fn status() -> serde_json::Value {
    let slot = lock();
    let (managed, owned) = match slot.as_ref() {
        Some(Slot::External) => (true, false),
        Some(Slot::Owned(_)) => (true, true),
        None => (false, false),
    };
    let pid = match slot.as_ref() {
        Some(Slot::Owned(service)) => service.child.id(),
        _ => 0,
    };
    drop(slot);

    serde_json::json!({
        "managed": managed,
        "owned": owned,
        "pid": pid,
        "dir": resolve_dir().map(|dir| display_path(&dir)),
        "online": probe(),
    })
}

/// 重启自己拉起的服务。微信重新登录后句柄失效，必须重启才能继续发送。
/// 复用外部服务时无法重启，返回提示让用户手动处理。
pub fn restart() -> Result<serde_json::Value, String> {
    if matches!(lock().as_ref(), Some(Slot::External)) {
        return Err("当前服务不是本应用启动的，请手动结束该进程后再试".to_string());
    }
    stop();
    STOPPING.store(false, Ordering::Release);
    start_blocking(false);
    Ok(status())
}

fn wait_ready() {
    let started_at = Instant::now();
    while started_at.elapsed() < READY_TIMEOUT {
        if probe() {
            eprintln!(
                "[wx-service] 服务就绪，耗时 {:.1}s",
                started_at.elapsed().as_secs_f32()
            );
            return;
        }
        thread::sleep(Duration::from_millis(250));
    }
    eprintln!("[wx-service] 等待服务就绪超时，详见 state/wx-push.log");
}

/// 探一次 /health，只有返回里带 `"ok": true` 才算自己的服务。
fn probe() -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PORT));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
    let request = b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).is_err() {
        return false;
    }
    let mut buffer = [0_u8; 512];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let text = String::from_utf8_lossy(&buffer[..read]);
    text.contains("\"ok\": true") || text.contains("\"ok\":true")
}

fn spawn(dir: &Path) -> Result<Service, String> {
    let python = python_path(dir).ok_or_else(|| {
        format!(
            "{} 下没有可用的 Python 虚拟环境（缺 .venv/Scripts/python.exe）",
            display_path(dir)
        )
    })?;

    let state_dir = dir.join("state");
    let _ = std::fs::create_dir_all(&state_dir);
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(state_dir.join("wx-push.log"))
        .map_err(|error| format!("无法写日志：{error}"))?;
    let stderr = stdout
        .try_clone()
        .map_err(|error| format!("无法复制日志句柄：{error}"))?;

    let mut command = Command::new(&python);
    command
        .arg("wx_server.py")
        .arg("--port")
        .arg(PORT.to_string())
        .current_dir(dir)
        // WorkBuddy 等工具会注入这些变量，会把 venv 的 import 路径搞乱
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .env_remove("NODE_OPTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let child = command
        .spawn()
        .map_err(|error| format!("启动 python 失败（{}）：{error}", display_path(&python)))?;

    #[cfg(windows)]
    let job = attach_job(&child);

    Ok(Service {
        child,
        #[cfg(windows)]
        job,
    })
}

/// 把子进程放进「句柄关闭即全部终止」的 Job Object。
#[cfg(windows)]
fn attach_job(child: &Child) -> isize {
    use std::mem::{size_of, zeroed};
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            eprintln!("[wx-service] 创建 Job Object 失败，退出时只能强制 kill");
            return 0;
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if configured == 0 {
            eprintln!("[wx-service] 配置 Job Object 失败");
            CloseHandle(job);
            return 0;
        }

        if AssignProcessToJobObject(job, child.as_raw_handle() as _) == 0 {
            eprintln!("[wx-service] 进程加入 Job Object 失败");
            CloseHandle(job);
            return 0;
        }

        job as isize
    }
}

/// 定位 wx-sender 目录。优先级：环境变量 → 数据目录里的指针文件 → 应用同级 → 工作目录。
fn resolve_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(value) = std::env::var("LOL_STATS_WX_SENDER") {
        let value = value.trim();
        if !value.is_empty() {
            candidates.push(PathBuf::from(value));
        }
    }

    if let Some(base) = data_dir() {
        // 单行文本，内容是 wx-sender 的绝对路径，方便重装/换盘后手工改
        if let Ok(text) = std::fs::read_to_string(base.join("wx-sender.txt")) {
            let pointer = text.trim();
            if !pointer.is_empty() {
                candidates.push(PathBuf::from(pointer));
            }
        }
        candidates.push(base.join("wx-sender"));
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("wx-sender"));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join("wx-sender"));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("wx-sender"));
    }

    candidates
        .into_iter()
        .find(|dir| dir.join("wx_server.py").is_file() && python_path(dir).is_some())
        .and_then(|dir| dir.canonicalize().ok().or(Some(dir)))
}

fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|base| PathBuf::from(base).join("lol-stats"))
}

fn python_path(dir: &Path) -> Option<PathBuf> {
    [
        ".venv/Scripts/python.exe",
        ".venv/bin/python",
        "venv/Scripts/python.exe",
    ]
    .iter()
    .map(|relative| dir.join(relative))
    .find(|path| path.is_file())
}

/// `canonicalize` 在 Windows 上会带 `\\?\` 前缀，日志里看着别扭。
fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}
