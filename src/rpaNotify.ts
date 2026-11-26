import { ref } from "vue"
import { runExternalCommand } from "./api"
import { readBooleanSetting, writeBooleanSetting } from "./toolSettings"

/**
 * 导出/自动截图之后，唤起影刀 RPA 把剪贴板里的截图发到微信。
 *
 * 影刀官方没有 CLI，但支持用协议方式唤起已发布的机器人：
 *   "<影刀安装目录>\ShadowBot.exe" shadowbot:Run?robot-uuid=<应用UUID>
 * 坐标里的 robot-uuid 在影刀客户端「应用 → 更多 → 应用信息」里复制；
 * 注意草稿/编辑中的流程无法被外部唤起，必须先「发布」。
 */

export const RPA_NOTIFY_ENABLED_KEY = "lol-stats.rpa.notify.enabled"
export const RPA_NOTIFY_COMMAND_KEY = "lol-stats.rpa.notify.command"
export const RPA_NOTIFY_KILL_FIRST_KEY = "lol-stats.rpa.notify.kill-first"

/** 影刀主进程名，残留进程会导致外部唤起不生效 */
const SHADOWBOT_PROCESS = "ShadowBot.exe"
const KILL_SETTLE_MS = 1500

export const rpaNotifyEnabled = ref(readBooleanSetting(RPA_NOTIFY_ENABLED_KEY, false))
export const rpaNotifyCommand = ref(localStorage.getItem(RPA_NOTIFY_COMMAND_KEY) || "")
export const rpaNotifyKillFirst = ref(readBooleanSetting(RPA_NOTIFY_KILL_FIRST_KEY, false))

export function setRpaNotifyEnabled(value: boolean) {
  rpaNotifyEnabled.value = value
  writeBooleanSetting(RPA_NOTIFY_ENABLED_KEY, value)
}

export function setRpaNotifyCommand(value: string) {
  rpaNotifyCommand.value = value
  localStorage.setItem(RPA_NOTIFY_COMMAND_KEY, value)
}

export function setRpaNotifyKillFirst(value: boolean) {
  rpaNotifyKillFirst.value = value
  writeBooleanSetting(RPA_NOTIFY_KILL_FIRST_KEY, value)
}

export function rpaNotifyReady() {
  return rpaNotifyEnabled.value && rpaNotifyCommand.value.trim().length > 0
}

function delay(ms: number) {
  return new Promise((resolve) => window.setTimeout(resolve, ms))
}

/**
 * 唤起影刀。失败会抛出异常，交由调用方决定怎么提示 —— 截图已经进剪贴板了，
 * 这一步失败不该影响导出本身的结果。
 */
export async function runRpaNotify() {
  const command = rpaNotifyCommand.value.trim()
  if (!command) throw new Error("未配置影刀命令")

  if (rpaNotifyKillFirst.value) {
    // 影刀开着的时候，外部唤起常常只是把窗口切到前台而不真的跑流程
    await runExternalCommand(`taskkill /f /im ${SHADOWBOT_PROCESS}`)
    await delay(KILL_SETTLE_MS)
  }

  await runExternalCommand(command)
}
