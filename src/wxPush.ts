import { ref } from "vue"
import { readBooleanSetting, writeBooleanSetting } from "./toolSettings"

/**
 * 本地微信推送：把截图直接 POST 给常驻的 wx_server.py，
 * 由它调用微信原生发送函数把图片投到指定群。
 *
 * 和影刀的区别：不碰剪贴板、不抢前台窗口，后台静默发送。
 * 代价是要在本机跑一个 Python 服务（见 wx-sender/README-wx-service.md）。
 */

export const WX_PUSH_ENABLED_KEY = "lol-stats.wxpush.enabled"
export const WX_PUSH_URL_KEY = "lol-stats.wxpush.url"
export const WX_PUSH_CHATS_KEY = "lol-stats.wxpush.chats"
export const WX_PUSH_TEXT_KEY = "lol-stats.wxpush.text"

export const DEFAULT_WX_PUSH_URL = "http://127.0.0.1:8756"

export type WxChat = { id: string; name: string }

function readChatIds(): string[] {
  try {
    const raw = JSON.parse(localStorage.getItem(WX_PUSH_CHATS_KEY) || "[]")
    if (Array.isArray(raw)) return raw.filter((item) => typeof item === "string" && item.trim())
  } catch {
    /* 旧的单值格式或损坏，回落到空 */
  }
  return []
}

export const wxPushEnabled = ref(readBooleanSetting(WX_PUSH_ENABLED_KEY, false))
export const wxPushUrl = ref(localStorage.getItem(WX_PUSH_URL_KEY) || DEFAULT_WX_PUSH_URL)
export const wxPushChatIds = ref<string[]>(readChatIds())
export const wxPushText = ref(localStorage.getItem(WX_PUSH_TEXT_KEY) || "")

export function setWxPushEnabled(value: boolean) {
  wxPushEnabled.value = value
  writeBooleanSetting(WX_PUSH_ENABLED_KEY, value)
}

export function setWxPushUrl(value: string) {
  wxPushUrl.value = value
  localStorage.setItem(WX_PUSH_URL_KEY, value)
}

export function setWxPushChatIds(ids: string[]) {
  wxPushChatIds.value = [...new Set(ids.filter((id) => id.trim()))]
  localStorage.setItem(WX_PUSH_CHATS_KEY, JSON.stringify(wxPushChatIds.value))
}

export function toggleWxPushChat(id: string, checked: boolean) {
  const next = new Set(wxPushChatIds.value)
  if (checked) next.add(id)
  else next.delete(id)
  setWxPushChatIds([...next])
}

export function setWxPushText(value: string) {
  wxPushText.value = value
  localStorage.setItem(WX_PUSH_TEXT_KEY, value)
}

export function wxPushReady() {
  return (
    wxPushEnabled.value &&
    wxPushUrl.value.trim().length > 0 &&
    wxPushChatIds.value.length > 0
  )
}

function baseUrl() {
  return wxPushUrl.value.trim().replace(/\/+$/, "")
}

/** 查询服务是否在线，以及它允许发往哪些聊天。 */
export async function probeWxPush(): Promise<{ ok: boolean; chats: WxChat[] }> {
  const response = await fetch(`${baseUrl()}/health`, { method: "GET" })
  if (!response.ok) throw new Error(`服务返回 ${response.status}`)
  const data = await response.json()
  return { ok: Boolean(data?.ok), chats: Array.isArray(data?.chats) ? data.chats : [] }
}

export type WxCandidate = {
  id: string
  name: string
  isGroup: boolean
  whitelisted: boolean
}

/** 让服务去读本机微信会话列表，返回所有可发的聊天（含是否已在白名单）。 */
export async function listWxCandidates(): Promise<WxCandidate[]> {
  const response = await fetch(`${baseUrl()}/available`, { method: "GET" })
  if (!response.ok) throw new Error(`服务返回 ${response.status}`)
  const data = await response.json()
  if (!data?.ok) throw new Error(data?.error || "服务拒绝列举聊天")
  return Array.isArray(data.candidates) ? data.candidates : []
}

/**
 * 保存白名单到 config.json。
 * 这是唯一会写服务端配置的接口，只在配置页显式点「保存」时调用。
 */
export async function saveWxChats(chats: WxChat[]): Promise<WxChat[]> {
  const response = await fetch(`${baseUrl()}/config/chats`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ chats }),
  })
  const data = await response.json().catch(() => null)
  if (!response.ok || !data?.ok) {
    throw new Error(data?.error || `保存失败（HTTP ${response.status}）`)
  }
  return Array.isArray(data.chats) ? data.chats : []
}

/**
 * 把 base64 PNG 推到所有勾选的微信群。
 * 返回实际成功的 chat 列表；有失败会连同错误一起返回，不整体抛错，
 * 免得一个群配置错了导致其它群也发不出去。
 */
export async function pushPngToWx(imageBase64: string, text?: string) {
  const targets = wxPushChatIds.value
  if (!targets.length) throw new Error("还没有勾选任何目标群")

  const body = {
    image: imageBase64,
    text: text ?? wxPushText.value.trim() ?? "",
  }

  const sent: string[] = []
  const failed: { chat: string; error: string }[] = []

  for (const chat of targets) {
    try {
      const response = await fetch(`${baseUrl()}/push`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ ...body, chat }),
      })
      const data = await response.json().catch(() => null)
      if (!response.ok || !data?.ok) {
        throw new Error(data?.error || `HTTP ${response.status}`)
      }
      sent.push(chat)
    } catch (error) {
      failed.push({ chat, error: error instanceof Error ? error.message : String(error) })
    }
  }

  if (!sent.length) {
    throw new Error(failed[0]?.error || "所有目标都发送失败")
  }
  return { sent, failed }
}
