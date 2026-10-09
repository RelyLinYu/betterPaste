import { mount } from "svelte";
import "../lib/theme.css";
import App from "./App.svelte";

// 关掉 WebView2 自带的右键菜单（另存为 / 查看源代码 / 刷新），设置面板不需要它。
window.addEventListener("contextmenu", (e) => e.preventDefault());

const app = mount(App, { target: document.getElementById("app")! });

export default app;
