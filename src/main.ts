import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './style.css'

// 主题初始化:localStorage 优先,否则跟随系统偏好(在 Vue 挂载前设置,避免闪烁)
const savedTheme = localStorage.getItem('trae-check-theme')
const initialTheme = savedTheme
  || (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
document.documentElement.dataset.theme = initialTheme

const app = createApp(App)
const pinia = createPinia()

app.use(pinia)
app.mount('#app')
