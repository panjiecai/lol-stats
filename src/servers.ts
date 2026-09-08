export type SearchServerOption = { id: string; label: string }

export const ALL_SEARCH_SERVER_ID = "ALL"
export const DEFAULT_SEARCH_SERVER_ID = "TENCENT_HN1"
export const LOCAL_SERVER_LABEL = "本地区服"

export const searchServerOptions: SearchServerOption[] = [
  { id: ALL_SEARCH_SERVER_ID, label: "不限" },
  { id: "TENCENT_HN1", label: "艾欧尼亚" },
  { id: "TENCENT_HN10", label: "黑色玫瑰" },
  { id: "TENCENT_NJ100", label: "联盟一区" },
  { id: "TENCENT_GZ100", label: "联盟二区" },
  { id: "TENCENT_CQ100", label: "联盟三区" },
  { id: "TENCENT_TJ100", label: "联盟四区" },
  { id: "TENCENT_TJ101", label: "联盟五区" },
  { id: "TENCENT_BGP2", label: "峡谷之巅" },
  { id: "TENCENT_PBE", label: "体验服" },
  { id: "TENCENT_PREPBE", label: "PREPBE" },
]

export function isKnownSearchServer(sgpServerId: string) {
  return searchServerOptions.some((option) => option.id === sgpServerId)
}

export function normalizeSearchServerId(sgpServerId?: string) {
  const normalized = sgpServerId?.trim().toUpperCase() || DEFAULT_SEARCH_SERVER_ID
  return isKnownSearchServer(normalized) ? normalized : DEFAULT_SEARCH_SERVER_ID
}

export function searchServerLabel(sgpServerId?: string) {
  if (!sgpServerId) return LOCAL_SERVER_LABEL
  return searchServerOptions.find((option) => option.id === sgpServerId)?.label || sgpServerId
}
