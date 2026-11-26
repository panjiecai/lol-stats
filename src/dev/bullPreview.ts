import { createApp } from "vue"
// 预览页也要显式引入全局徽章样式，保证看到的和 app 里一致
import "../bullChip.css"
import BullPreview from "./BullPreview.vue"

createApp(BullPreview).mount("#bull-preview")
