/**
 * 监控面板的浏览器烟测入口（`npm run dev` 后打开 monitor-dev.html）。
 *
 * 这里把 Tauri IPC 换成假数据，用来在浏览器里真实渲染 MonitorPanel，
 * 验证组件挂载、轮询调度、新对局检测与状态切换 —— 不依赖游戏客户端。
 * 为了让「发现新对局」能自然发生，每个账号前两次查询返回旧对局，
 * 第三次开始推进 gameId。自动导出默认关闭，避免走需要真实对局明细的导出链路。
 */
import { createApp, h, ref } from "vue"
import MonitorPanel from "../components/MonitorPanel.vue"
import { notifyKey, type AppToast, type ToastPayload } from "../notifications"

const DEMO_ACCOUNTS = [
  { query: "迷路的小丑#1234", sgpServerId: "TENCENT_HN1" },
  { query: "阿狸不撒娇#6666", sgpServerId: "TENCENT_BGP2" },
  { query: "ShadowFox#8888", sgpServerId: "TENCENT_HN10" },
]

localStorage.setItem(
  "lol-stats.monitor.settings",
  JSON.stringify({ tickMs: 800, autoExportDefault: false, autoStart: false }),
)
localStorage.setItem(
  "lol-stats.monitor.accounts",
  JSON.stringify(
    DEMO_ACCOUNTS.map((item, index) => ({
      id: `demo-${index}`,
      query: item.query,
      sgpServerId: item.sgpServerId,
      enabled: true,
      autoExport: false,
      baselineGameId: null,
      baselineCreation: 0,
      foundCount: 0,
      lastFoundAt: 0,
      label: item.query,
    })),
  ),
)

const BASE_CREATION = Date.now() - 3_600_000
const callCount = new Map<string, number>()

function game(gameId: number, gameCreation: number) {
  return {
    gameId,
    gameCreation,
    gameDuration: 1800,
    queueId: 2400,
    championId: 1,
    win: true,
    kills: 8,
    deaths: 3,
    assists: 12,
    damageToChampions: 23580,
  }
}

const internals = {
  invoke: async (cmd: string, args: Record<string, unknown>) => {
    if (cmd === "search_player") {
      const query = String(args.query ?? "")
      const times = (callCount.get(query) ?? 0) + 1
      callCount.set(query, times)
      // 第三次查询开始给出「刚打完」的新对局，触发监控面板的发现逻辑
      const advanced = times >= 3
      const [gameName, tagLine = ""] = query.split("#")
      return {
        summoner: {
          puuid: `puuid-${query}`,
          gameName,
          tagLine,
          displayName: query,
          internalName: query,
        },
        depthRequested: 20,
        depthLoaded: 2,
        summary: {},
        championStats: [],
        recentGames: [
          game(advanced ? 2000 + times : 1000, advanced ? BASE_CREATION + 600_000 * times : BASE_CREATION),
          game(999, BASE_CREATION - 600_000),
        ],
      }
    }
    if (cmd === "copy_png_to_clipboard") return null
    throw new Error(`dev 预览未 mock 的命令：${cmd}`)
  },
}

;(window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = internals

const toasts = ref<AppToast[]>([])
let toastId = 0

function notify(payload: ToastPayload) {
  toastId += 1
  const id = toastId
  toasts.value = [...toasts.value.slice(-4), { ...payload, id }]
  console.log(`[toast:${payload.kind}] ${payload.title}`, payload.message || "")
  return id
}

const app = createApp({
  setup() {
    return () =>
      h("div", { class: "dev-root" }, [
        h(MonitorPanel, { champions: {}, gameAssets: {} as never }),
        h(
          "div",
          { class: "toast-log" },
          toasts.value.map((toast) =>
            h("div", { class: `toast-item ${toast.kind}`, key: toast.id }, [
              h("strong", toast.title),
              toast.message ? h("span", toast.message) : null,
            ]),
          ),
        ),
      ])
  },
})

app.provide(notifyKey, notify)
app.mount("#monitor-dev")
