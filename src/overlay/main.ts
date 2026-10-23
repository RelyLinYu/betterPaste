import { mount } from "svelte";
import "../lib/theme.css";
import App from "./App.svelte";

// WebView2 默认会弹出浏览器右键菜单（另存为 HTML / 查看源代码 / 刷新…），
// 对一个原生风格的小工具来说非常出戏，直接关掉。
window.addEventListener("contextmenu", (e) => e.preventDefault());

const app = mount(App, { target: document.getElementById("app")! });

export default app;
