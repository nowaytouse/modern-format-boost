import { createApp } from 'vue'
import App from './App.vue'
import './styles/variables.css'
import { useI18n } from './composables/useI18n'

// Default dark theme (official plugin style)
document.documentElement.setAttribute('data-theme', 'dark')

const app = createApp(App)

// 🌍 Global i18n setup - Best Practice
// Create a global i18n instance that can be used throughout the app
const i18n = useI18n()

// Method 1: Global property for template usage ($t)
app.config.globalProperties.$t = i18n.t

// Method 2: Provide for composable usage (inject)
app.provide('i18n', i18n)
app.provide('t', i18n.t)

app.mount('#app')
