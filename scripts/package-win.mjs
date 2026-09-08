import { spawn } from "node:child_process"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const rootDir = path.resolve(__dirname, "..")

const tauriCliJs = path.join(
  rootDir,
  "node_modules",
  "@tauri-apps",
  "cli",
  "tauri.js",
)

function log(message) {
  console.log(`[package] ${message}`)
}

function runShellStep(step) {
  return new Promise((resolve, reject) => {
    log(`开始 ${step.label}`)
    const child = spawn(step.command, {
      cwd: rootDir,
      stdio: "inherit",
      shell: true,
    })
    child.on("exit", (code) => {
      if (code === 0) {
        resolve()
        return
      }
      reject(new Error(`${step.label} 失败，退出码 ${code ?? "unknown"}`))
    })
    child.on("error", reject)
  })
}

// 直接以参数数组调用 tauri.js（不经 shell），避免 cmd.exe 对 JSON 里双引号的转义问题。
function runTauriBuild() {
  return new Promise((resolve, reject) => {
    log("开始 Tauri release 打包 (NSIS)，跳过前端重建（dist 已最新）")
    const configOverride = JSON.stringify({ build: { beforeBuildCommand: "" } })
    const child = spawn(
      process.execPath,
      [
        tauriCliJs,
        "build",
        "--config",
        configOverride,
      ],
      { cwd: rootDir, stdio: "inherit" },
    )
    child.on("exit", (code) => {
      if (code === 0) {
        resolve()
        return
      }
      reject(new Error(`Tauri 打包失败，退出码 ${code ?? "unknown"}`))
    })
    child.on("error", reject)
  })
}

async function main() {
  // 1) 先单独构建前端，产出最新 dist。
  //    说明：在普通终端（无文件删除守卫）里直接 `npm run package:win` 即可一次跑完。
  //    但在 WorkBuddy 沙箱里，本脚本经由 node 再 spawn 出的 `npm run build`
  //    子进程会被重新套上安全删除守卫，vite emptyDir 清空 dist/assets(数百文件) 会被拦。
  //    沙箱内请改用两条“顶层命令”手动执行：
  //        npm run build
  //        npx tauri build --config '{"build":{"beforeBuildCommand":""}}'
  await runShellStep({ label: "前端构建", command: "npm run build" })

  // 2) 用 --config 把 beforeBuildCommand 置空，跳过前端重建（dist 已是最新），
  //    直接编译 Rust release + 打 NSIS 安装包。
  //    注意：这里用直接 argv 调 tauri.js（不经 shell），在沙箱内也能正常跑。
  await runTauriBuild()

  log("全部完成，安装包已生成到 src-tauri/target/release/bundle/nsis/")
}

main().catch((error) => {
  console.error(error instanceof Error ? error.message : error)
  process.exit(1)
})
