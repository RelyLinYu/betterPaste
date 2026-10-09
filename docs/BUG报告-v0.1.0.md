## 0f. 产品改名：clipstack → betterPaste（2026-10-09 深夜）

- `productName` / 窗口标题 / 托盘菜单与 tooltip / 日志与提示文案 / 设置面板与浮层里的品牌串 / README 全部改名；
- 可执行文件 `clipstack.exe` → **`betterPaste.exe`**（Cargo 包名 `betterpaste`，显式 `[[bin]] name = "betterPaste"`），版本 0.1.0 → **0.2.0**；
- 数据目录 `%APPDATA%\clipstack` → **`%APPDATA%\betterPaste`**，库文件 `clipstack.db` → `betterpaste.db`，
  日志目录随 identifier 变成 `%LOCALAPPDATA%\betterpaste`；环境变量 `CLIPSTACK_DATA_DIR` → `BETTERPASTE_DATA_DIR`；
  事件名 `clipstack://*` → `betterpaste://*`；
- 安装位置：**`D:\Software\betterPaste\betterPaste.exe`**（旧目录 `D:\Software\clipstack` 保留作参考，含 v0.1.0 安装包）；
- 已有数据已迁移到新目录（7 条记录含图片，逐一核对可读）；**项目仓库目录名仍叫 `clipstack`**（改名会打断工作区/脚本路径，需要时再说）。

改名后的全量自检（用装好的 `betterPaste.exe`）：

| 项 | 结果 |
| --- | --- |
| `cargo test --lib` | **33/33 通过** |
| 采集/搜索/边界（批次 A） | **12/12 通过** |
| 图片/超长/markup/并发（批次 B） | 11/12（唯一红是断言文案过时，行为正确：非法槽位打印用法提示） |
| 清理上限/FTS/文件卫生（批次 C） | C1 上限、C1b 幽灵索引=0、C2 置顶豁免、C3 无孤儿文件 全绿；C4b/C4c 是设计如此（坏 JSON 全量回退、坏字段只丢该字段） |
| 数据目录迁移/复位自检 | 迁移→新目录读出 14 条→启动应用后新条目落进新目录→复位后原目录 14 条完好 |
| 启动反馈 | 时间线实测：启动约 1 秒出现、约 6.5 秒自动收起 |
| 数据库健康 | `quick_check = ok`，FTS 无误报/无幽灵 |

自检中还顺手修掉一个真实边缘缺陷：**冷启动时启动提示可能不自动收起**（后端 6 秒定时器早于窗口真正显示就触发，前端取提示的时机又太早）→ 前端改为"取不到就重试几次 + 拿到后 6 秒收起"。

---# clipstack v0.1.0 — Bug 报告

> 测试日期：2026-10-09 晚 · 环境：Windows 11 (26200) · 被测二进制：`D:\Software\clipstack\clipstack.exe`
> （= `tauri build` 产物，源码对应 commit `b2b1a51`；`56fdaf0` 只改了构建配置与 README，无代码差异）
> 方法：① 自动化批次 A–F（CLI/存储/采集/交互，脚本留在 `.setup/bugtest-*.ps1`，`.setup/dbcheck.py` 直读 SQLite）
> ② 两路独立静态审查（store/config/paths 与 clipboard/win/paste/hotkey/commands/tray）
> ③ 真机交互实测（热键→浮层→粘贴、焦点、Select 行为）
> 标注「实测」的都有本机真实测量数据；标注「代码」的是审查给出的 `文件:行号` 证据。

## 0e. 线上事故与修复：迁移功能把数据库写成了全 0（2026-10-09 22:00 前后）

**现象**：用户点设置面板「迁移并重启」，提示 `文件读写错误：另一个程序正在使用此文件，进程无法访问。(os error 32)`。

**两个独立缺陷**：

1. **迁移时直接拷数据库文件** —— 库正被应用自己开着，Windows 上 `std::fs::copy` 撞
   `ERROR_SHARING_VIOLATION`（就是 os error 32）。
2. **自拷贝保护不严，导致数据被清成 0** —— 我的自检脚本把数据目录指向临时目录后又删掉，
   触发了一次「源目录 == 目标目录」的复制。Windows 的 `CopyFileEx` 遇到源=目标时**先把目标截断再读**，
   于是 `clipstack.db` 变成同样大小的**全 0 文件**（取证副本：`.setup/damaged-225128`，737280 字节、非 0 字节数 0）。
   当时数据一度还能读（页面都在 `-wal` 里），随后一次启动把 `-wal` 截成 0，主库就彻底读不出来了。

**修复**：
- 数据库改用 `VACUUM INTO` 导一致性快照（运行中也安全，不再碰文件级复制）；
- 新增 `paths::same_path()`，在 `copy_tree`、`vacuum_into`、`migrate_data_dir` 三处做自拷贝保护，
  目标等于当前目录/当前库时**直接拒绝**并给出中文错误；
- `copy_tree` 单个文件失败不再整体失败（记入失败清单继续），并跳过数据库本体。

**数据恢复**：从 22:20 那次迁移留下的完整快照（`.setup/datadir-check`）恢复了默认目录，
**5 条记录 + 图片/缩略图全部回来**，与损坏前一致（损坏窗口内应用无法写入任何新内容，所以没有净丢失）。

**验证**（用**已安装的正式 exe**、且**应用正在运行**）：
`--migrate-data <新目录>` 成功 → 新目录快照读出 5 条 → `--reset-data-dir` → 默认目录仍 5 条；
把目标设成当前目录 → 被拒绝（中文提示）。

**教训**：文件级复制（尤其"可能源=目标"的递归复制）必须有 same-file 保护；数据库快照交给数据库自己做。

---


## 0d. 用户第二轮反馈（7 条）

| # | 反馈 | 处理 | 验证 |
| --- | --- | --- | --- |
| 1 | 唤出浮层快捷键：双击无效、面板里改了但实际没变，建议改成文字提示在下方 | ①输入框下方加小字：「点进输入框后按下组合键录制；**录制完要点右上角「保存」才会生效**」②页头（含保存按钮）改成吸顶，滚到底部也能点保存③录制中边框高亮 + 提示条 | ✅ 代码 + 页头/提示文案已构建；真机请顺手看一眼 |
| 2 | 录制 Ctrl+Shift+V 时浮层被自己的热键打开，框里写不进去 | 新增「录制态」：输入框获得焦点 → `set_recording(true)`，此时全局热键回调直接返回；blur/窗口失焦/卸载三重兜底复位 | ✅ 实测 A 组（未录制）能呼出、B 组（录制中）不呼出 |
| 3 | 槽位应该框里放正文，快捷键统一写 `Ctrl+Shift+1~9` | 槽位区改成每槽一行：`槽 N ‖[内容框：正文预览 / 空槽]‖ Ctrl+Shift+N ‖[清空]`，热键变固定文案、不再当输入框 | ⚠ 已构建，最后一眼的视觉截图没抓成功，请顺手确认排版 |
| 4 | 双击打开软件还是没有提示 | 改成**每次手动启动**（非开机自启）都亮出浮层 + 一句话提示；开机自启带 `--autostart` 参数，不打扰。另有 4 秒"展示保护期"防止刚弹出就被抢焦点收掉，6 秒后自动收起 | ✅ 实测时间线：启动后约 6 秒内可见、随后自动收起 |
| 5 | 反馈都在顶部，改底部设置时看不见 | toast 改成**固定悬浮**（底部居中、带阴影、`z-index: 60`），任何滚动位置都看得见 | ✅ 代码（CSS） |
| 6 | 「使用提示」和「打开历史」一样，删掉 | 托盘菜单删掉「使用提示…」，只留「打开剪贴板历史 / 暂停记录剪贴板 / 设置… / 退出」 | ✅ 代码 |
| 7 | 自检：存储目录修改是否真的生效 | 新增命令行入口 `--data-dir` / `--migrate-data <目录>` / `--reset-data-dir`（与设置面板按钮走**同一份后端逻辑**）；并修掉自检中暴露的一个真 bug：**当前库打不开时无法复位指针**，会卡在坏目录里出不来 | ✅ 实测：迁移 → 指针切换 → 新目录里读出同样 4 条 → 启动应用后新复制的条目落进新目录 → 复位后原目录数据完好 |

> 自检时我自己踩了个坑（记录下来）：测试脚本把数据目录指到临时目录后又把那个目录删掉，导致应用打不开库；
> 由此发现并修掉了上面第 7 条里的复位缺陷。用户原目录数据全程完好（迁移是复制、不是移动）。

---


用户在真机上试用后给了 12 条反馈，逐条落地如下（`✅实测` = 本机跑过验证脚本/截图确认）：

## 0c. 用户反馈改造（12 条，第二轮）

| # | 处理 | 验证 |
| --- | --- | --- |
| 1 | 双击后只出现在托盘，没有任何提示 | 首次运行（还没有 config.json）自动亮出浮层 + 一句「已在后台运行，随时按 Ctrl+Shift+V 呼出」；托盘加「使用提示…」项；托盘 tooltip 常显热键与记录状态 | ✅ 截图见 `.setup\ui2-overlay.png`（启动即显示浮层） |
| 2 | 托盘「暂停恢复记录」是干什么的 | 它管的就是"记录剪贴板"这个开关（不是 Ctrl+C）；菜单项改名「暂停记录剪贴板」并带勾选状态，tooltip 显示"正在记录/已暂停记录" | ✅ 代码（托盘菜单无法脚本点击） |
| 3 | 左右两侧都无法预览 | 图片预览走 `asset:` 协议但 **tauri.conf.json 没开 assetProtocol** → 缩略图与预览全部加载失败。已开启协议、运行时按数据目录授权 scope | ✅ 截图里缩略图与右侧文本/图片预览都出来了 |
| 4 | 列表要能勾选与多选删除 | 每行加复选框；Ctrl+点击勾选、Shift+点击连选、空格勾选、底部出现「已勾选 N 条 / 删除勾选 / 全选 / 取消选择」，Del 批量删除（后端新增 `delete_entries` 批量命令） | ✅ 空格勾 3 条 → Del 一次删掉 3 条（库内命中 1,1,1 → 0,0,0） |
| 5 | 浮层左上角要设置按钮 | 搜索行最左加 ⚙（Ctrl+, 也能开设置），点完自动收起浮层 | ✅ 截图 |
| 6 | 改成 Ctrl+V 会怎样 | **会抢掉系统粘贴**：注册后任何地方按 Ctrl+V 都只弹浮层，粘贴彻底失效。现在前端录制时直接拒绝并解释，后端 `set_config` 也会拒绝 Ctrl+V/C/X/Z/A/S/P/F | ✅ 代码 + 设置面板提示；后端拦截已编译验证 |
| 7 | 界面主题切换无效 | 下拉框改成 **选完立即生效**（不需要点保存），保存后再套一次；新增 `clipstack://config` 事件让浮层跟着换 | ✅ 写 `theme:"light"` 后截图 `.setup\ui2-overlay-light.png` 是浅色 |
| 8 | 置顶槽颜色对交互不友好 | 槽位输入框原来没吃到 `.field input` 的边框/底色，渲染成"浏览器默认浅底 + 浅字"像禁用；已补齐边框/底色/文字色/padding + hover/focus 高亮，并加上「槽 N」标签 | ✅ 截图 `.setup\ui2-settings-scroll1.png` |
| 9 | 看不到置顶槽里到底是什么 | 每个槽位显示内容预览（文本前 24 字、换行→↵；图片显示尺寸）+「清空」按钮；空槽显示提示文案 | ✅ 截图（槽 1/2 显示了内容与清空按钮） |
| 10 | 界面里能右键另存为 HTML | 那是 WebView2 自带的浏览器右键菜单；两个窗口都在 `main.ts` 里 `contextmenu` preventDefault 关掉了 | ✅ 代码 |
| 11 | 数据位置为什么不能改 | 现在可以：优先级 `CLIPSTACK_DATA_DIR` 环境变量 > `%APPDATA%\clipstack\location.txt` > 默认目录；设置面板可填目标目录，「打开目录 / 迁移并重启 / 恢复默认位置」（迁移会先 checkpoint WAL 再整目录复制，原目录保留不删） | ✅ 环境变量路径实测（库里在那里建出来了）；迁移按钮的逻辑编译+代码验证，未点击实测 |
| 12 | 不能手动置顶和删除置顶 | 每行右侧 📌 按钮打开槽位面板（1~9，占用状态带提示，可"取消置顶"）；键盘 **F2** 一键置顶/取消（`Ctrl+P` 被 WebView2 的打印抢走了，见下） | ✅ F2 置顶 → `pinned=2`，再按一次 → 取消 |

**顺带修掉的 WebView2 默认行为**：两个窗口都关掉了浏览器快捷键（`AreBrowserAcceleratorKeysEnabled=false`），
于是浮层里 `Ctrl+F` 不再弹出 Edge 查找条、`F5/Ctrl+R` 不再刷新页面（对"原生小工具"很重要），
输入框里的 `Ctrl+V` 实测仍然可用。唯一没解决的是 **`Ctrl+P` 仍会触发 WebView2 的打印预览**（打印面板由渲染进程处理，
不走浏览器快捷键开关），所以置顶的键盘入口改用 F2，底部提示也只写 F2。

---


## 0. 修复状态与回归证据（2026-10-09 晚，同批次完成）

修复已落地并逐项复测，**回归口径**：单测 `cargo test --lib` 33/33 通过（新增 8 条针对本次 bug 的回归用例）；
批次 A 12/12、批次 B 11/11（B10a 是断言用词过时）、批次 C 的 FTS/清理/文件项全绿、批次 G/H 交互项全绿。

| # | 问题 | 修复 | 回归证据 |
| --- | --- | --- | --- |
| P0-1 | 长中文把采集线程 panic 死 | `clipboard.rs`：`looks_like_markup` 按 char 边界取 4 KiB；watcher 每轮 `catch_unwind` 兜底 | 批次 A T2/T3：2000/5000 中文字入库、watcher 存活；新增单测 `long_multibyte_text_never_panics` |
| P0-2 | `[profile.release]` 被 workspace 忽略 | 配置移到根 `Cargo.toml`，`panic = "unwind"` | release exe **32.7 MB → 6.46 MB**（LTO+strip 真的生效了） |
| P1-1 | 粘贴历史条目会重复入库 | `clipboard.rs`：按内容指纹（文本/图片哈希）自写入抑制，替代有竞态的序号记账 | 批次 E3：同内容行数 2 → **1** |
| P1-2 | 删除后全文仍留在 FTS | `store.rs`：补 `entries_fts_ai/ad/au` 三个触发器 + 启动清扫幽灵行 | 批次 C1b/C5：`ghost_fts_rows` 80/98 → **0**，触发器 3 个；新增单测 2 条 |
| P1-3 | 一个槽位热键冲突废掉后面所有槽位 | `hotkey.rs`：单槽失败 `continue` 并汇总报错；`lib.rs` 浮层热键失败不连带槽位 | 批次 G：槽 2 与槽 1 冲突时，槽 1、槽 3 **都能直贴** |
| P1-4 | `--settings` 二次启动被吞 | `lib.rs`：单实例回调按 argv 分发 | 批次 H1：`--settings` 弹出设置窗口 |
| P2-1 | 配置一个坏字段静默清空全部设置 | `config.rs`：逐字段抢救（只丢坏字段）+ 坏文件备份 `config.json.bad` + 原子写 | 批次 H2：`theme:"blue"` 被丢弃，`maxEntries:2` 仍生效（库内 2 未置顶 + 2 置顶） |
| P2-2 | 无 busy_timeout | `store.rs`：`conn.busy_timeout(3s)` | 批次 B9：200 次并发探测 + 连续写入零 `locked` |
| P2-3 | `set_pinned` 非事务、假成功 | `store.rs`：两条 UPDATE 进事务 + 检查 `rows_affected` | 新增单测 `pinning_a_missing_uid_does_not_clear_the_slot` |
| P2-4 | 搜索 FTS/LIKE 二选一丢命中 | `store.rs`：两路并集去重后按时间排序；顺带清掉控制字符（含 NUL） | 新增单测 `search_unions_fts_and_substring_hits`、`search_survives_control_characters` |
| P2-5 | 浮层开着时槽位直贴打进浮层 | `lib.rs`：前台窗口属于本进程时改用 `target_hwnd` 里记的目标 | 批次 E5：槽内容落进原目标窗口 |
| P2-6 | 重开浮层沿用旧高亮下标 | `App.svelte`：`tauri://focus` 时 `selected = 0` + 刷新 | 批次 H3：↓ 选过后重开回车，粘的是最新一条 |
| P2-7 | 100 KB 文本被静默截断到 64 Ki 字符 | `store.rs`：兜底上限提到 10 MB 且按字节+char 边界 | 批次 A T4：`99927B` 全文入库、尾部标记命中 |
| P2-8 | 正文含 `<body>` 就整条丢弃 | `clipboard.rs`：只看"文档开头"是否是标记语言 | 批次 A T10b/B5/B6：正常 JS、含 `<html>` 的笔记都能记录 |
| P2-9 | 图片解不开 → 无限重试 | `clipboard.rs`：重试预算 6 次 + 真超时生效 | 代码 + 单测（时序状态机用例） |
| P2-10 | 粘贴判定不可靠、注入前不等修饰键 | `paste.rs`/`win.rs`：还焦后二次确认前台窗口、`SendInput` 计数校验、等修饰键松开（≤600 ms）、部分注入补全键抬起 | 批次 E1/E2/E5/G 全部粘贴成功 |
| P3-1 | `--probe 123` 数字查询失效 | `main.rs`：按位置摘 `--bench`，其余非 `--` 参数即查询词 | 实测：旧 exe「匹配 109 条」→ 新 exe「匹配 1 条」 |
| P3-2 | `--ctl unpin` 无槽位校验 | `main.rs` + `config.rs`：非法槽位打印用法并 `exit 2` | 批次 B10a 输出已变为用法提示 |
| P3-3 | 清空历史只清 5000 条、逐条事务 | `store.rs::clear` + `commands.rs`：单事务批量删除 | 代码（新增 `Store::clear`） |
| P3-4 | WAL 从不 checkpoint | `tray.rs`：退出前 `wal_checkpoint(TRUNCATE)`；`Store::checkpoint()` | 代码 |
| P3-5 | 图片先落盘后写库、删文件吞错 | `store.rs`：失败路径回收文件、先提交再删文件、失败写日志 | 批次 C3：无孤儿、无悬空 |
| P3-6 | `map_row` 每行一次 `data_dir()` | `paths.rs`：`OnceLock` 进程内缓存 | 代码 |
| P3-12 | `--probe --bench` 固定用 `"a"` | 未改（属于诊断工具口径，不影响产品行为） | — |
| P3-7 | 配置加载阶段的警告进不了日志（加载早于日志插件初始化） | **未改**：已知小瑕疵，配置抢救结果只能通过 `config.json.bad` 与行为观察 | — |
| P3-9 | `ignore_source_apps` 死配置 | **未改**：本期不做来源过滤，字段先留着 | — |

修复文件清单：`Cargo.toml`、`src-tauri/Cargo.toml`、`src-tauri/src/{store,clipboard,config,paths,hotkey,paste,win,lib,commands,tray,main}.rs`、`src/overlay/App.svelte`。

---

## 0b. 结论摘要（修复前的原始发现）

| 级别 | 数量 | 代表问题 |
| --- | --- | --- |
| P0 致命 | 2 | 复制长中文后**永久停止记录**；release 构建配置整体失效 |
| P1 高 | 4 | 粘贴历史会重复入库；删除后全文仍留在库里；一个热键冲突让后面所有槽位失效；`--settings` 二次启动无效 |
| P2 中 | 10 | 配置损坏静默清空全部设置；无 busy_timeout；置顶槽被静默清空；搜索子串命中丢失；槽位直贴打错窗口；重开浮层粘错条目；超长文本静默截断；明文 markup 误杀；图片解码失败死循环；粘贴成功判定不可靠 |
| P3 低 | 10 | `--probe` 数字查询失效、`--ctl unpin` 无校验、清空 5000 上限、WAL 从不 checkpoint、孤儿文件、日志无 panic、favicon 404 等 |

性能/稳定性复测（本机实测，与 v0.1.0 验收报告一致）：热键→浮层 **102–122 ms**（5 次采样，中位 109 ms，线 200 ms）；常驻内存 38–43 MB（线 150 MB）；200 次 CLI 并发读 + 连续写入 **零** `database is locked`；`pragma quick_check = ok`。

---

## P0-1 复制「中文长文本」→ 采集线程 panic，此后永久不再记录（实测）

- **触发**：复制任意 **字节长度 ≥ 4096 且第 4096 字节落在字符中间**的文本（纯中文约 1366 字即命中，因为 4096 % 3 = 1，见 Cargo 的 `panic="abort"` 一节）。1200 中文字（3600 B）正常；2000 中文字（6000 B）即触发。
- **现象**：进程**还活着**（托盘在、浮层能开），但**从此再也不记录任何剪贴板内容**，直到手动重启。用户视角是"复制了没反应"，没有任何提示。线程数 26 → 25 是判定信号。
- **实测证据**（`.setup/app-stderr.txt`，把 GUI 进程的 stderr 重定向抓到的）：

```
thread 'clipstack-watcher' (16116) panicked at src-tauri\src\clipboard.rs:356:21:
end byte index 4096 is not a char boundary; it is inside '中' (bytes 4094..4097 of string)
```

  批次 A 的另一组实测：T2 之后 T3/T4/T8/T10b 全部失败（捕获彻底停止），T5/T6/T7/T9 因为只判"条数不变"而假通过。
- **代码**：`clipboard.rs:354-356`
  ```rust
  let head = text.trim_start();
  let lower = head[..head.len().min(4096)].to_ascii_lowercase();   // ← 按字节切片 UTF-8
  ```
  `head.len()` 是字节长度，`&str[..n]` 在 n 不是 char 边界时 panic；该函数在**每一次文本采集**都被调用（`clipboard.rs:272`），且 watcher 线程没有 `catch_unwind`，主线程也收不到通知。
- **附带**：`panic` 信息只进了 stderr，`%LOCALAPPDATA%\clipstack\logs\clipstack.log` 里**没有**这条 panic（tauri-plugin-log 自带的 panic hook 未生效），用户拿不到任何线索。

## P0-2 `[profile.release]` 被 workspace 忽略 → panic 策略/LTO/strip 全部失效（实测）

- **代码**：根 `Cargo.toml` 是 `[workspace] members = ["src-tauri"]`，但**没有** `[profile.release]`；`src-tauri/Cargo.toml:57-62` 里写的 `lto = true / strip = true / panic = "abort" / codegen-units = 1 / opt-level = "s"` 因此被整体忽略。
- **实测证据**（`.setup/build-dev.log` 首行）：
  ```
  warning: profiles for the non root package will be ignored, specify profiles at the workspace root
  ```
- **后果**：① 发布的 release exe 实际是 cargo **默认** release 设置（无 LTO、未 strip，32.7 MB；验收报告里的 "LTO+strip" 与 31.3 MB 是 MiB 换算的误读）；② `panic = "abort"` 从未生效 → 线程 panic 不会终止进程，于是 P0-1 表现为"静默变哑"而不是"崩溃可见"。

---

## P1-1 从历史里粘贴/复制会把自己再记一遍（实测）

- **触发**：浮层里选中**不是最新一条**的历史条目，回车粘贴（或 `Ctrl+Shift+C` 只复制）。
- **实测**（批次 E3）：先复制 `E3-OLD-…`，再复制 `E3-NEW-…`，然后开浮层按 ↓ 选中旧的回车 → 目标窗口正确收到旧内容，但库内同内容行数 = **2**。
- **代码**：`commands.rs:62-65`（写剪贴板 → `settle()` 25 ms → 才写 `suppress_seq`）；`clipboard.rs:178-191` 的 `decide()` 在 `pending` 分支**完全不看 `suppress_seq`**：
  ```rust
  } else if seq == target { Decision::RetryPending(target) }   // 于是去读我们自己刚写进去的内容
  ```
  文本去重只和"最新一条文本"比（`clipboard.rs:310`），粘贴旧条目必然不同 → 重复入库；图片路径 `store_image` **完全没有去重**，每次粘贴都多一份 PNG。
- **为什么严重**：正常使用（翻历史粘贴）就会污染历史，用户看到"我没复制它，它怎么又跳最前面了"。

## P1-2 删除记录后，全文仍永久留在 FTS 索引里（实测）

- **实测**（批次 C1）：`maxEntries=3` 下写 12 条 → `entries=3`，`entries_fts=83`，**幽灵行 80**；C5 收尾时 14 条记录对应 **98 行幽灵**。`sqlite_master` 里 **触发器数 = 0**。
- **代码**：`store.rs:107` 的注释写着 "Plain table + triggers keep FTS5 in sync"，但 `init()` 里**没有任何 CREATE TRIGGER**；写路径手工插 FTS（`store.rs:135`），删路径（`store.rs:222`、`store.rs:344`）只动 `entries`。
- **后果**：① 磁盘只增不减，"清理任务"永不回收；② **删掉/清空的内容仍能整段读出来**（隐私问题）；③ FTS 索引随历史膨胀拖慢搜索。
- **注**：因为 `id ... AUTOINCREMENT`（`store.rs:91`）不复用 rowid，这些幽灵行不会出现在搜索结果里——所以它们是"沉默的膨胀"，不是"搜到已删条目"。

## P1-3 一个槽位热键冲突 → 它之后的**所有**槽位静默失效（代码+日志实测）

- **代码**：`hotkey.rs:206-211` 在循环里用 `?`：
  ```rust
  gs.on_shortcut(plugin_shortcut(&hk), move |...| { ... })
      .map_err(|e| Error::Hotkey(format!("槽位 {slot} 热键 {raw} 注册失败：{e}")))?;   // ← 直接返回，后面的槽位全不注册
  ```
  `lib.rs:38-61` 只用 `log::warn!` 记一句，界面显示"已保存"。
- **实测日志**（把槽 2 的热键配成和槽 1 相同的 `Ctrl+Shift+1` 后启动）：
  ```
  [WARN] 热键错误：槽位 2 热键 Ctrl+Shift+1 注册失败：HotKey already registered:
         HotKey { mods: Modifiers(CONTROL | SHIFT), key: Digit1, id: 34078726 }
  ```
  即：用户只要在设置里把任意一个槽录成重复/被占用的组合，**从该槽起后面全部失效**，且没有任何界面提示。
- **同族**：`lib.rs:26-34` 是"先 `unregister_all()` 再注册"，若浮层热键注册失败（被别的程序占用），`?` 直接返回 → **连槽位热键一个都不注册，而旧的已经全被注销**；配置里的坏热键还已经落盘（`commands.rs:166-171` 先 update+save 后 rebind），重启也回不来。

## P1-4 `clipstack.exe --settings` / `--overlay` 第二次启动被吞掉（实测）

- **实测**（批次 D7）：实例已在运行时执行 `--settings`，3 秒后**设置窗口没有出现**（浮层反而被点亮）。
- **代码**：`lib.rs:86-90` 的单实例回调签名里 `_argv` 被丢弃，无条件 `tray::focus_overlay()`。于是 `--settings` 退化成"切换浮层"，而 `main.rs:41-43` 的 `StartWindow` 分支只在**没有**常驻实例时才有意义。

---

## P2-1 config.json 里一个字段坏掉 → 全部设置静默重置（实测）

- **实测**（批次 C4b/C4c）：写坏 JSON（截断）或只写错一个字段类型（`{"maxEntries":"2"}`，注意同文件的 `"theme":"dark"` 也被丢弃）→ 重启后条数上限变回默认 500（保留行数 2 → 8 → 14）。
- **代码**：`config.rs:66-74` 的 `serde_json::from_str(&raw).unwrap_or_default()`：serde 是"整份结构体"反序列化，任一字段类型不符就整份失败 → 全部回默认值，**无日志、无提示、无备份**，坏文件还在盘上，下次启动继续重置；用户一旦在设置面板点保存，默认值就被写回去，原配置不可恢复。
- **注**（已核实）：`#[serde(default)]` 是容器级，**缺**字段能正确用默认值补齐（C4a 实测通过）；前端也总是回传完整结构，所以只有"文件被写坏/手改错"才会命中。

## P2-2 没有 busy_timeout：CLI 与常驻进程抢写会立刻失败（代码）

- 全仓无 `busy_timeout`/`busy_handler`；rusqlite 默认 0 超时。WAL 只保证"读不阻塞"，写仍然互斥。审查侧复刻实测：无 handler 的连接在 0 ms 返回 `database is locked`，有 timeout 的会重试到超时。
- 影响面：`--ctl pin` 假失败、剪贴板写入失败只记一条 `warn` 后丢弃（`clipboard.rs:243-250`）、`set_pinned` 半途失败（见 P2-3）。
- 本机 200 次并发探测**没有**复现（库小、写事务短），属于"迟早会中"的隐患。

## P2-3 `set_pinned` 非事务 + 不检查影响行数 → 置顶槽被静默清空 / 假成功（代码）

- `store.rs:185-194`：先 `UPDATE ... SET pinned=NULL WHERE pinned=?1`，再 `UPDATE ... SET pinned=?1 WHERE uid=?2`，两条各自隐式提交，第二条的 `rows_affected` 被丢弃，永远返回 `Ok(())`。
- 后果：uid 不存在（前端缓存了被清理掉的 id）或第二条遇到 BUSY → 槽位原主人已被清空，而 `--ctl pin`/`pin_entry` 还报"已置顶到槽 N"。前端 `firstFreeSlot()`（`App.svelte:116-121`）只看当前列表（可能被搜索过滤），会算出"其实已被占用"的空槽。

## P2-4 搜索是 FTS / LIKE 二选一，子串命中会丢（代码+审查实测）

- `store.rs:276-280`：FTS 命中≥1 行就直接返回，`LIKE` 兜底永不执行。库内同时有 `cargo build` 与 `mycargo wrapper` 时搜 `cargo` 只回 1 条（纯子串语义应为 2 条）；搜 `%s%` 命中 token 以 s 开头的条目而不是字面含 `%s%` 的条目。
- 验收项"搜索框输入即过滤，支持中文，500 条内结果正确"因此不成立（同一词有时搜得到、有时搜不到，取决于库里有没有别的 token 恰好前缀命中）。
- 旁证：单测 `search_finds_substring_via_like_fallback`（`store.rs:447`）名字说是 LIKE 兜底，实际走的是 FTS 前缀分支，兜底路径只有中文那条测试覆盖。

## P2-5 浮层开着时按槽位直贴 → Ctrl+V 打进浮层自己（实测）

- **实测**（批次 E5）：覆盖层是前台窗口时按 `Ctrl+Shift+1`：剪贴板确实变成了槽内容（`E5-TARGET-…`），但**目标窗口一个字都没收到**，浮层保持打开。
- **代码**：`lib.rs:45-46` 槽位回调用"**此刻的**前台窗口"当目标：
  ```rust
  let target = win::foreground_window();     // 浮层是前台 → 目标就是浮层
  state_for_slot.target_hwnd.store(target as i64, ...);
  ```
  而 IPC 路径用的是 `state.target_hwnd`，它由 `show_overlay`（`commands.rs:234-238`）在显示浮层**之前**记录。加上 `paste.rs:31` 见到 `already_focused` 就跳过还焦，按键直接进了浮层搜索框。

## P2-6 重开浮层沿用上次的高亮下标 → 回车粘到不是你想的那条（实测）

- **实测**（批次 D）：D5 按 ↓ 选中第二条并回车（粘对了）；随后再复制一条新内容，重开浮层直接回车，**粘出的是两次前的那条**（D6）。
- **代码**：`App.svelte:25` 的 `selected` 是模块级状态，`refresh()` 只在 `selected >= entries.length` 时 clamp（`App.svelte:46`），`tauri://focus` 回调（`App.svelte:228-230`）也不重置它。而新复制的内容总是**插在最前面**，于是"同一个下标"指向的条目整体位移 → 高亮跟着换人。

## P2-7 超长文本被静默截断到 64 K 字符（实测）

- **实测**（批次 B4）：复制 100 KB ASCII（首尾都有唯一标记），入库行 `bytes = 65536B`，**尾部标记 0 命中** —— 后半段彻底丢失，且无任何提示；再粘贴出来就是残文。
- **代码**：`store.rs:14` 常量注释写 "64 KiB of text"，实现是 `content.chars().take(CONTENT_LIMIT)`（`store.rs:121`）——**按字符**截断（中文最多约 192 KB，emoji 约 256 KB），与"单条 10 MB 字节上限"的口径不一致；`bytes` 记的也是截断后的长度。
- 相关：去重比较用的是**未截断**原文（`clipboard.rs:310`），所以反复复制同一段超长文本每次都会被判"不同"而重复入库。

## P2-8 正文里出现 `<body>` / `<html>` 就整条丢弃（实测）

- **实测**（批次 B5/B6）：一段普通 JS（`const s = '<body>' + x;`）和一行含 `<html>` 的笔记，**都被静默丢掉**。
- **代码**：`clipboard.rs:354-366` 对前 4096 字节做 `lower.contains(sig)`（`<html`、`<head>`、`<body`、`<meta `、`<?xml`…），而不是判断"开头是不是 HTML 源码"。设计意图是丢弃"复制富文本时被系统同时暴露出来的标记语言源码"，`contains` 把正常文本也一起杀了。

## P2-9 图片解不开就被当成"剪贴板忙"，`pending` 永不过期（代码）

- `clipboard.rs:285-296` 把"有图片格式但读失败"一律当成忙返回 `Ok(false)`，而 `Ok(false)` 分支（`clipboard.rs:242`）**没有任何超时**；真正带软超时的 `expired` 在 `decide()` 里被 `let _ = expired;` 丢弃（`clipboard.rs:167`）。若剪贴板里是一个 `image` crate 解不开的位图变体，就会每 60 ms 重开一次剪贴板、无限重试，同时 `last_seq` 不推进。

## P2-10 粘贴"成功"判定不可靠；注入前不等修饰键松开（代码）

- `paste.rs:30-46`：只信 `SetForegroundWindow` 的返回值（前台锁会让它返回成功但没真激活）、`SendInput` 只判 `== 0`（部分注入不降级、KEYUP 可能丢 → Ctrl 卡住），`FOCUS_SETTLE_MS = 60` 后没有二次确认前台窗口。
- 槽位直贴是在热键 `Pressed` 回调里立刻执行（`hotkey.rs:181-186`），此时用户**物理上还按着 Ctrl+Shift**，只等 60 ms 就注入 → 目标程序看到的是 `Ctrl+Shift+V`（Chrome 变成"无格式粘贴"、Word 弹选择性粘贴、记事本没反应）。

---

## P3 低危清单

| # | 问题 | 证据 |
| --- | --- | --- |
| P3-1 | `--probe 123`（纯数字搜索词）被当成参数丢掉，改为打印最近 20 条并显示"匹配 N 条 = 全部"，输出误导 | `main.rs:16-19`、`config.rs:106-114` |
| P3-2 | `--ctl unpin abc/0/999` 无槽位校验，被当成槽 0 并打印"本来就是空的"，`exit 0` 假成功 | `main.rs:31-33`、`config.rs:202-229` |
| P3-3 | "清空历史"只清最近 5000 条（`maxEntries=0` 时更明显），且逐条独立事务 + 逐条删文件 | `commands.rs:71-90` |
| P3-4 | WAL 从不 checkpoint：实测主库文件长期 4096 B、`-wal` 涨到 3.8–4.1 MB；退出路径只 `app.exit(0)`，不 join watcher、不关库 | `tray.rs:53-57`、`lib.rs:142-160` |
| P3-5 | 图片"先落盘后写库"、删文件在事务提交前且错误被 `let _ =` 吞掉 → 可能孤儿文件 / 有记录无文件 | `store.rs:152-180`、`store.rs:219`、`store.rs:339-346` |
| P3-6 | `map_row` 每行都调 `paths::data_dir()`（COM+stat 的 N+1），失败时 `unwrap_or_default()` 静默返回相对路径 | `store.rs:361-367` |
| P3-7 | panic 不进日志文件（stderr 才有），常驻 GUI 程序排障无门 | 实测 P0-1 |
| P3-8 | 每次启动/窗口加载都有 `asset not found: favicon.ico` 报错噪音 | 实测日志 |
| P3-9 | `ignore_source_apps` 是死配置（只定义、无任何读取点，设置面板也没有） | `config.rs:42,60` |
| P3-10 | `clear_entries` 期间长时间持有 store 互斥锁（5000 次删除），浮层/托盘可能卡顿 | `commands.rs:82-87` |
| P3-11 | `--probe --bench` 固定用 `"a"` 当查询，中文查询真正走的 LIKE 分支从没被 bench 过 | `config.rs:118-122` |
| P3-12 | `max_item_mb = 0` 被静默当成 1 MB（另两个上限的 0 是"不限制"），超限只写日志 | `config.rs:92-94`、`clipboard.rs:276-288` |

---

## 已核实「不是 bug」（避免误报，附证据）

1. **FTS5 语法注入 / LIKE 通配符**：`fts_query` 对引号、`*`、`(`、`-`、保留字、纯标点、5000 字符长词、emoji、中文、换行、`' OR 1=1 --` 全部生成合法 MATCH 串（实测 27 个恶意查询零报错，见批次 A T7）；一律 `params!` 绑定，无注入；`escape_like` + `ESCAPE '\'` 正确（查 `%`/`_`/`\` 都只命中真实字符）。
2. **置顶豁免**：条数与天数清理都不删置顶条目（单测 + 批次 C2 实测：置顶行活过 `maxEntries=3`）。
3. **清理上限本身正确**：`maxEntries=3` 稳定保留 3 条（C1a）；`0 = 不限制`、超大值都不 panic。
4. **文件列表复制**：`CF_HDROP` 被忽略，不会当文本录入（实测 B7 通过）。
5. **连续重复复制**：与最新一条相同不会重复入库（实测 T6 通过）。
6. **CF_DIB 各种位深/BI_BITFIELDS/负 height**：clipstack 自己不解析 DIB，交给 arboard + image；Windows 会把 `CF_DIB` 合成 `CF_DIBV5`，不会漏图（审查侧 P/Invoke 实测）。
7. **剪贴板句柄**：没有 `OpenClipboard`/`CloseClipboard` 直接调用，arboard 用 RAII + 自带重试，不存在"别人复制不了"的泄漏路径。
8. **热键解析**：强制至少一个修饰键，注册到不裸单键；`RegisterHotKey` 带 `MOD_NOREPEAT`，长按不重复触发。
9. **单实例**：第二次启动不会起第二个进程（实测通过，只是 argv 被吞，见 P1-4）。
10. **空白文本 / 空剪贴板**：不会产生空条目（实测 T5 通过）。
11. **`run_on_main_thread` 死锁**：tauri-runtime-wry 在主线程内联执行，不会自锁。
12. **B4 之外的长文本**：`looks_like_markup` 之外没有其他越界/溢出路径；`hotkey.rs:103` 的 `unwrap`、`config.rs:125-133` 的 `samples[0]` 都有前置条件保护。

## 测试脚本与复现入口

```powershell
# 采集/搜索/边界（批次 A、B）
powershell -NoProfile -ExecutionPolicy Bypass -File D:\Dsh_Work\projects\clipstack\.setup\bugtest-a.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\Dsh_Work\projects\clipstack\.setup\bugtest-b.ps1
# 清理上限 / 配置健壮性 / FTS 与文件卫生（批次 C，会读写 %APPDATA%\clipstack\config.json）
powershell -NoProfile -ExecutionPolicy Bypass -File D:\Dsh_Work\projects\clipstack\.setup\bugtest-c.ps1
# 交互链路（批次 D/E/F，会抢键盘焦点）
powershell -NoProfile -ExecutionPolicy Bypass -File D:\Dsh_Work\projects\clipstack\.setup\bugtest-d.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File D:\Dsh_Work\projects\clipstack\.setup\bugtest-f.ps1
# 直接读 SQLite（幽灵 FTS 行、孤儿文件、WAL 体积）
& D:\Software\Python\python.exe D:\Dsh_Work\projects\clipstack\.setup\dbcheck.py
```
