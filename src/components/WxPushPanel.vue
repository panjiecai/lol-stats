<script setup lang="ts">
import { computed, inject, onMounted, ref } from "vue"
import {
  AlertTriangle,
  Ban,
  CheckCircle2,
  LoaderCircle,
  RefreshCw,
  Save,
  Search,
  Send,
  Trash2,
  Users,
} from "lucide-vue-next"
import { notifyKey } from "../notifications"
import {
  DEFAULT_WX_PUSH_URL,
  listWxCandidates,
  probeWxPush,
  pushPngToWx,
  saveWxChats,
  setWxPushChatIds,
  setWxPushEnabled,
  setWxPushText,
  setWxPushUrl,
  toggleWxPushChat,
  wxPushChatIds,
  wxPushEnabled,
  wxPushText,
  wxPushUrl,
  type WxChat,
  type WxCandidate,
} from "../wxPush"

/**
 * 监控页里的微信推送配置：服务连通 → 列出本机所有可发聊天 → 勾选白名单 → 保存。
 *
 * 两条独立的闸门：
 *   1. 服务端 config.json 的 chats（真正决定能不能发，保存后生效）
 *   2. 这里的勾选（决定这一台前端要不要发往某个群）
 * 所以「保存白名单」和「勾选发送」是两件事，UI 上分开摆，避免误以为是同一个开关。
 */

const notify = inject(notifyKey, () => 0)

const urlDraft = ref(wxPushUrl.value)
const textDraft = ref(wxPushText.value)

const connecting = ref(false)
const saving = ref(false)
const loadingCandidates = ref(false)
const serviceOnline = ref(false)

const candidates = ref<WxCandidate[]>([])
const whitelist = ref<WxChat[]>([])
/** 白名单编辑态：从当前白名单初始化，保存后同步回来 */
const whitelistDraft = ref<WxChat[]>([])
const search = ref("")

const defaultUrl = DEFAULT_WX_PUSH_URL

const filteredCandidates = computed(() => {
  const keyword = search.value.trim().toLowerCase()
  if (!keyword) return candidates.value
  return candidates.value.filter(
    (item) =>
      item.name.toLowerCase().includes(keyword) || item.id.toLowerCase().includes(keyword),
  )
})

const whitelistIds = computed(() => new Set(whitelistDraft.value.map((item) => item.id)))

const whitelistDirty = computed(() => {
  const a = whitelistDraft.value.map((item) => item.id).sort().join(",")
  const b = whitelist.value.map((item) => item.id).sort().join(",")
  return a !== b
})

const groupCount = computed(() => candidates.value.filter((item) => item.isGroup).length)

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

/**
 * 服务由应用主进程随启动拉起，冷启动时要解密微信本地库，
 * 头几秒探活必然失败，所以自动连接要带重试。
 */
async function connectService(options: { silent?: boolean; retries?: number } = {}) {
  if (connecting.value) return
  setWxPushUrl(urlDraft.value.trim() || defaultUrl)
  connecting.value = true
  const retries = options.retries ?? 0
  try {
    let health: Awaited<ReturnType<typeof probeWxPush>> | null = null
    let lastError: unknown = null
    for (let attempt = 0; attempt <= retries; attempt += 1) {
      try {
        health = await probeWxPush()
        break
      } catch (error) {
        lastError = error
        if (attempt < retries) await new Promise((resolve) => setTimeout(resolve, 600))
      }
    }
    if (!health) throw lastError
    serviceOnline.value = health.ok
    whitelist.value = health.chats
    whitelistDraft.value = health.chats.map((item) => ({ ...item }))
    if (!options.silent) {
      notify({
        kind: "success",
        title: "微信服务在线",
        message: `已配置 ${health.chats.length} 个可发送目标`,
        duration: 6000,
      })
    }
    void loadCandidates()
  } catch (error) {
    serviceOnline.value = false
    notify({
      kind: "error",
      title: "连不上微信服务",
      message: `${errorMessage(error)}；服务随应用启动，可能还在初始化，稍后点「连接服务」重试`,
      duration: 9000,
    })
  } finally {
    connecting.value = false
  }
}

async function loadCandidates() {
  if (loadingCandidates.value) return
  loadingCandidates.value = true
  try {
    candidates.value = await listWxCandidates()
    if (!candidates.value.length) {
      notify({
        kind: "warning",
        title: "没读到聊天列表",
        message: "微信可能没登录，或会话库读不到；可手动在下方添加",
        duration: 8000,
      })
    }
  } catch (error) {
    notify({
      kind: "error",
      title: "读取聊天列表失败",
      message: errorMessage(error),
      duration: 8000,
    })
  } finally {
    loadingCandidates.value = false
  }
}

function addToDraft(candidate: WxCandidate) {
  if (whitelistIds.value.has(candidate.id)) return
  whitelistDraft.value = [
    ...whitelistDraft.value,
    { id: candidate.id, name: candidate.name },
  ]
}

function removeFromDraft(id: string) {
  whitelistDraft.value = whitelistDraft.value.filter((item) => item.id !== id)
}

async function saveWhitelist() {
  if (saving.value) return
  if (!whitelistDraft.value.length) {
    notify({
      kind: "error",
      title: "白名单不能为空",
      message: "至少保留一个可发送目标",
      duration: 7000,
    })
    return
  }
  saving.value = true
  try {
    const saved = await saveWxChats(whitelistDraft.value)
    whitelist.value = saved
    whitelistDraft.value = saved.map((item) => ({ ...item }))
    // 本地勾选里已经不存在的目标要清掉，否则会一直发失败
    const allowed = new Set(saved.map((item) => item.id))
    const stale = wxPushChatIds.value.filter((id) => !allowed.has(id))
    if (stale.length) {
      setWxPushChatIds(wxPushChatIds.value.filter((id) => allowed.has(id)))
    }
    void loadCandidates()
    notify({
      kind: "success",
      title: "白名单已保存",
      message: `现在只允许发送到 ${saved.length} 个目标${stale.length ? `，已移除 ${stale.length} 个失效勾选` : ""}`,
      duration: 8000,
    })
  } catch (error) {
    notify({ kind: "error", title: "保存白名单失败", message: errorMessage(error), duration: 9000 })
  } finally {
    saving.value = false
  }
}

function onToggleSend(id: string, event: Event) {
  toggleWxPushChat(id, (event.target as HTMLInputElement).checked)
}

function onEnabledChange(event: Event) {
  setWxPushEnabled((event.target as HTMLInputElement).checked)
}

function persistUrl() {
  setWxPushUrl(urlDraft.value.trim() || defaultUrl)
}

function persistText() {
  setWxPushText(textDraft.value)
}

async function sendTestCard() {
  if (!wxPushChatIds.value.length) {
    notify({
      kind: "error",
      title: "还没有勾选发送目标",
      message: "在「发送到此群」列勾选至少一个",
      duration: 7000,
    })
    return
  }
  try {
    const canvas = document.createElement("canvas")
    canvas.width = 640
    canvas.height = 200
    const ctx = canvas.getContext("2d")
    if (!ctx) throw new Error("无法创建画布")
    ctx.fillStyle = "#0a1428"
    ctx.fillRect(0, 0, canvas.width, canvas.height)
    ctx.fillStyle = "#c8aa6e"
    ctx.font = "bold 30px 'Microsoft YaHei'"
    ctx.fillText("微信推送连通测试", 40, 90)
    ctx.fillStyle = "#7a8ba0"
    ctx.font = "18px 'Microsoft YaHei'"
    ctx.fillText(new Date().toLocaleString("zh-CN"), 40, 135)

    const base64 = canvas.toDataURL("image/png").split(",")[1]
    const result = await pushPngToWx(base64, textDraft.value.trim())
    notify({
      kind: "success",
      title: "测试图已发出",
      message: `成功 ${result.sent.length} 个${result.failed.length ? `，失败 ${result.failed.length} 个` : ""}`,
      duration: 8000,
    })
  } catch (error) {
    notify({ kind: "error", title: "发送测试图失败", message: errorMessage(error), duration: 9000 })
  }
}

onMounted(() => {
  if (wxPushUrl.value.trim()) void connectService({ silent: true, retries: 8 })
})
</script>

<template>
  <section class="wx-panel">
    <header class="wx-panel-head">
      <div>
        <div class="eyebrow">WeChat Push</div>
        <h3>微信推送配置</h3>
      </div>
      <span class="wx-status" :class="serviceOnline ? 'on' : 'off'">
        <component :is="serviceOnline ? CheckCircle2 : AlertTriangle" :size="13" />
        {{ serviceOnline ? "服务在线" : "服务离线" }}
      </span>
    </header>

    <p class="wx-note">
      截图后由本机服务在后台直接发到微信群：不占剪贴板、不抢前台窗口。
      服务随应用自动启动，关闭应用即停止。
    </p>

    <div class="wx-row">
      <label class="wx-field">
        <span>服务地址</span>
        <input
          v-model="urlDraft"
          type="text"
          spellcheck="false"
          :placeholder="defaultUrl"
          @change="persistUrl"
        />
      </label>
      <label class="wx-toggle">
        <input type="checkbox" :checked="wxPushEnabled" @change="onEnabledChange" />
        <span>启用推送</span>
      </label>
      <button class="wx-btn" :disabled="connecting" @click="connectService()">
        <LoaderCircle v-if="connecting" :size="14" class="spin" />
        <RefreshCw v-else :size="14" />
        {{ connecting ? "连接中" : "连接服务" }}
      </button>
    </div>

    <div class="wx-columns">
      <div class="wx-col">
        <div class="wx-col-head">
          <span>本机聊天（{{ candidates.length }}，群 {{ groupCount }}）</span>
          <button class="wx-mini" :disabled="loadingCandidates" @click="loadCandidates">
            <LoaderCircle v-if="loadingCandidates" :size="12" class="spin" />
            <RefreshCw v-else :size="12" />
            刷新
          </button>
        </div>
        <label class="wx-search">
          <Search :size="13" />
          <input v-model="search" type="text" placeholder="按群名或 id 过滤" />
        </label>
        <div class="wx-list">
          <button
            v-for="item in filteredCandidates"
            :key="item.id"
            class="wx-list-item"
            :class="{ added: whitelistIds.has(item.id) }"
            :disabled="whitelistIds.has(item.id)"
            @click="addToDraft(item)"
          >
            <Users v-if="item.isGroup" :size="13" />
            <Ban v-else :size="13" />
            <span class="name">{{ item.name }}</span>
            <code>{{ item.id }}</code>
          </button>
          <p v-if="!filteredCandidates.length" class="wx-empty">
            {{ candidates.length ? "没有匹配的聊天" : "还没读到聊天列表，点「刷新」或先连接服务" }}
          </p>
        </div>
      </div>

      <div class="wx-col">
        <div class="wx-col-head">
          <span>允许发送白名单（{{ whitelistDraft.length }}）</span>
          <button
            class="wx-btn primary"
            :disabled="saving || !whitelistDirty"
            @click="saveWhitelist"
          >
            <Save :size="13" />
            {{ saving ? "保存中" : whitelistDirty ? "保存" : "已保存" }}
          </button>
        </div>
        <div class="wx-list">
          <div v-for="item in whitelistDraft" :key="item.id" class="wx-list-item static">
            <input
              type="checkbox"
              :checked="wxPushChatIds.includes(item.id)"
              @change="onToggleSend(item.id, $event)"
            />
            <span class="name">{{ item.name }}</span>
            <code>{{ item.id }}</code>
            <button class="wx-remove" title="从白名单移除" @click="removeFromDraft(item.id)">
              <Trash2 :size="12" />
            </button>
          </div>
          <p v-if="!whitelistDraft.length" class="wx-empty">白名单为空，从左边选一个加进来</p>
        </div>
        <p class="wx-hint">
          勾选 = 这台软件往哪些群发；保存 = 服务端允许哪些群。两个都通过才会真正发出。
        </p>
      </div>
    </div>

    <div class="wx-row footer">
      <label class="wx-field grow">
        <span>附带文字（可选）</span>
        <input
          v-model="textDraft"
          type="text"
          spellcheck="false"
          placeholder="留空则只发图"
          @change="persistText"
        />
      </label>
      <button class="wx-btn" @click="sendTestCard">
        <Send :size="14" />
        发送测试图
      </button>
    </div>
  </section>
</template>

<style scoped>
.wx-panel {
  border: 1px solid #dfe6e2;
  border-radius: 12px;
  padding: 16px 18px;
  background: #fbfdfc;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.wx-panel-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.wx-panel-head h3 {
  margin: 2px 0 0;
  font-size: 15px;
  color: #253831;
}

.wx-status {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  padding: 3px 9px;
  border-radius: 999px;
}

.wx-status.on {
  background: #e7f6ee;
  color: #1f7a4d;
}

.wx-status.off {
  background: #fdeeee;
  color: #b3453c;
}

.wx-note {
  margin: 0;
  font-size: 12px;
  color: #63736f;
  line-height: 1.6;
}

.wx-note code {
  background: #eef2f0;
  padding: 1px 5px;
  border-radius: 4px;
  font-size: 11px;
}

.wx-row {
  display: flex;
  align-items: flex-end;
  gap: 10px;
  flex-wrap: wrap;
}

.wx-row.footer {
  border-top: 1px solid #e8eeeb;
  padding-top: 12px;
}

.wx-field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 240px;
}

.wx-field.grow {
  flex: 1;
}

.wx-field > span {
  font-size: 12px;
  color: #63736f;
}

.wx-field input {
  padding: 7px 10px;
  border: 1px solid #dfe6e2;
  border-radius: 8px;
  font-size: 12.5px;
  font-family: Consolas, "Courier New", monospace;
}

.wx-toggle {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  color: #3c4f48;
  padding-bottom: 8px;
}

.wx-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 7px 13px;
  border: 1px solid #cfdad5;
  border-radius: 8px;
  background: #fff;
  color: #33463f;
  font-size: 12.5px;
  cursor: pointer;
}

.wx-btn:hover:not(:disabled) {
  border-color: #a9c2b8;
  background: #f4f9f6;
}

.wx-btn:disabled {
  opacity: 0.55;
  cursor: default;
}

.wx-btn.primary {
  background: #2f7d5a;
  border-color: #2f7d5a;
  color: #fff;
}

.wx-btn.primary:hover:not(:disabled) {
  background: #276a4c;
}

.wx-columns {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 14px;
}

@media (max-width: 900px) {
  .wx-columns {
    grid-template-columns: 1fr;
  }
}

.wx-col {
  display: flex;
  flex-direction: column;
  gap: 8px;
  min-width: 0;
}

.wx-col-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  font-size: 12px;
  color: #4a5d56;
  font-weight: 600;
}

.wx-mini {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 3px 8px;
  border: 1px solid #dfe6e2;
  border-radius: 6px;
  background: #fff;
  font-size: 11.5px;
  color: #55706a;
  cursor: pointer;
}

.wx-search {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 9px;
  border: 1px solid #dfe6e2;
  border-radius: 8px;
  background: #fff;
  color: #8a9a94;
}

.wx-search input {
  border: 0;
  outline: 0;
  flex: 1;
  font-size: 12px;
  background: transparent;
}

.wx-list {
  border: 1px solid #e8eeeb;
  border-radius: 8px;
  background: #fff;
  max-height: 260px;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}

.wx-list-item {
  display: flex;
  align-items: center;
  gap: 7px;
  padding: 6px 9px;
  border: 0;
  border-bottom: 1px solid #f1f5f3;
  background: transparent;
  text-align: left;
  font-size: 12px;
  color: #3c4f48;
  cursor: pointer;
  width: 100%;
}

.wx-list-item:last-child {
  border-bottom: 0;
}

.wx-list-item:hover:not(.added):not(.static) {
  background: #f4f9f6;
}

.wx-list-item.added {
  opacity: 0.45;
  cursor: default;
}

.wx-list-item.static {
  cursor: default;
}

.wx-list-item .name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 600;
}

.wx-list-item code {
  font-size: 10.5px;
  color: #8a9a94;
  max-width: 45%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.wx-remove {
  border: 0;
  background: transparent;
  color: #b07b76;
  cursor: pointer;
  padding: 2px;
  display: flex;
}

.wx-remove:hover {
  color: #c0392b;
}

.wx-empty {
  margin: 0;
  padding: 14px 10px;
  text-align: center;
  font-size: 11.5px;
  color: #91a09a;
}

.wx-hint {
  margin: 0;
  font-size: 11px;
  color: #8a9a94;
  line-height: 1.6;
}

.spin {
  animation: wx-spin 1s linear infinite;
}

@keyframes wx-spin {
  to {
    transform: rotate(360deg);
  }
}
</style>
