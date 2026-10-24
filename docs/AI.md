# 让 AI 操控 OCR Studio

> 这份文档写给两种人：想让本机 AI 帮自己干活的用户，和要维护这套接口的开发者。
> 最后更新：2026-10-24（工具从 17 个补到 35 个，加了「动手」能力）

---

## 1. 它是什么

OCR Studio 跑着一个本机 HTTP 服务（默认 `http://127.0.0.1:8848`）。除了给脚本用的
业务接口，另有一套**给 AI 用的接口**：`/ai/v1/tools`（能干什么）和 `/ai/v1/tool`
（干一件事）。

和「把接口文档丢给 AI」的区别：

| | 直接给业务接口 | `/ai/v1/tool` |
|---|---|---|
| 一次动作要调几个接口 | 常常 3 个（截图 → 认字 → 自己算坐标） | 1 个（`find_text` 直接给屏幕坐标） |
| 返回值 | `{code,msg,data}`，AI 要自己解读 | `summary`（人话）+ `data`（结构化） |
| 出错 | `1002` / `500` 这类码 | 一句话说清「下一步该做什么」 |
| 安全 | 无 | 开关 + 连接码 + 限流 + 幂等 + 审计 |

**边界（重要）**：服务只绑 `127.0.0.1`。跑在你电脑上的 AI（编程助手、本地脚本、
本地大模型）能调；**网页版 AI 在云端，访问不到 localhost**。

---

## 2. 打开它

1. 程序里进「设置 → AI 控制」，打开开关（默认关闭）。
2. 复制连接码（形如 `A1B2-C3D4-E5F6-7A8B`）。**每个请求都要带**：
   ```
   Authorization: Bearer A1B2-C3D4-E5F6-7A8B
   ```
3. 想让 AI 自己摸清能做什么：点「一键复制给 AI 的提示词」，整段粘给它。

连接码可以随时在面板上重置（旧码立刻失效）。它存在 `src-tauri/config.json`，
这个文件不进 git。

---

## 3. 两个端点

```
GET  /ai/v1/tools     → { ok, summary, data:{ tools:[{name,summary,risk,args}] } }
POST /ai/v1/tool      → { ok, summary, data, error }
```

调一个工具：

```bash
curl -s http://127.0.0.1:8848/ai/v1/tool \
  -H "Authorization: Bearer A1B2-C3D4-E5F6-7A8B" \
  -H "Content-Type: application/json" \
  -d '{"tool":"find_text","args":{"keyword":"确定"}}'
```

```json
{
  "ok": true,
  "summary": "找到「确定」，中心点在 (962, 731)，可以直接 click_at。",
  "data": {
    "found": true, "count": 1,
    "matches": [{ "text": "确定", "center_x": 962, "center_y": 731, "x": 940, "y": 716, "width": 44, "height": 30, "score": 0.998 }],
    "origin_x": 0, "origin_y": 0
  }
}
```

### 状态码

| 码 | 含义 | 怎么办 |
|---|---|---|
| 401 | 连接码不对 | 去设置页复制 |
| 403 | 开关没开 | 去设置页打开 |
| 400 | 参数错 / 工具执行失败 | 读 `error`，它写了下一次该怎么做 |
| 409 | 同一个 `request_id` 正在处理 | 等一会儿再来 |
| 429 | 超过 120 次/分钟 | 等一下 |

### 幂等

写操作带上 `request_id`，同一个值重复提交只会执行一次（缓存 200 条）：

```json
{"tool":"click_at","args":{"x":962,"y":731},"request_id":"confirm-20261024-1"}
```

---

## 4. 坐标约定（最容易踩的一条）

**所有返回的坐标都是屏幕绝对坐标**，可以直接喂给 `click_at` / `move_mouse`。

- `origin_x` / `origin_y` = 这次取图的左上角在屏幕上的位置。
- 传本地图片时是 `0,0`；截屏时是虚拟屏原点；截窗口时是窗口左上角（含边框）。
- 多显示器时原点可能是负数，所以**不要自己按图片尺寸反推**，用返回值。
- 传了 `region` 的话，返回的坐标已经加回去了，仍然可以直接点。

---

## 5. 工具全表（35 个）

### 5.1 只读（10 个）—— 只是看

| 工具 | 干什么 |
|---|---|
| `get_status` | 服务在不在跑、端口、引擎档位、推理设备、模型目录 |
| `list_endpoints` | 20 个业务端点的路径与方法（写调用代码前先查） |
| `list_engines` | 引擎能力（认字/检测/提示找图）与模型根目录 |
| `get_config` | 完整配置 |
| `get_logs` | 最近运行日志（`limit` 1-500） |
| `list_targets` | 已经教过的中文目标名 |
| `list_windows` | 当前所有可见窗口：标题、hwnd、位置、谁在前台 |
| `list_models` | 模型库每项装没装、多大、正在下载的进度 |
| `check_update` | 有没有新版本（只查不装） |
| `wait` | 等 1-10000 毫秒（点完等界面加载） |

### 5.2 写（11 个）—— 会读屏幕

| 工具 | 干什么 |
|---|---|
| `screenshot` | 截全屏 / 截窗口 / 截一块，存成文件并返回路径与原点 |
| `ocr_image` | 认字，返回整段文本 + 每行屏幕坐标 |
| `find_text` | **先认字再筛**：找某段文字在哪（支持 `/正则/`） |
| `find_target` | 按**教过的中文名**找界面元素 |
| `find_image` | 模板找图（拿小图在大图/屏幕里比像素） |
| `detect_objects` | 通用目标检测（类别按名字过滤，如「人,车」） |
| `read_table` | 表格 → HTML + TSV + 单元格坐标 |
| `read_qrcode` | 二维码 / 条形码 |
| `teach_target` | 教一个新目标：参考图 + 框 + 中文名 |
| `forget_target` | 忘掉一个教过的名字 |
| `focus_window` | 把某个窗口切到前台（最小化的先还原；用 AttachThreadInput 绕前台锁） |

### 5.3 危险（14 个）—— 会动你的电脑或改服务

| 工具 | 干什么 |
|---|---|
| `move_mouse` | 移动光标（悬停展开菜单） |
| `click_at` | 点击（左/右/中，`count:2` = 双击） |
| `drag_mouse` | 从 A 按住拖到 B（分 12 步移动，控件才认得出是拖拽） |
| `scroll` | 滚轮（不给坐标就滚鼠标当前位置） |
| `type_text` | 打字，**支持中文**（Unicode 注入） |
| `press_key` | 组合键：`enter`、`ctrl+s`、`alt+f4`、`ctrl+shift+esc` |
| `install_model` | 后台下载模型，立刻返回，用 `list_models` 看进度 |
| `remove_model` | 卸载模型（删 `models/<dir>`） |
| `server_start` / `server_stop` | 启停 HTTP 服务 |
| `set_port` / `set_level` / `set_device` / `set_threshold` | 改端口 / 模型档 / 推理设备 / 找图阈值 |

**明确不提供**（不注册，不是运行时拦截）：退出程序、开机自启、AI 开关本身与连接码。
这三件事必须由人在界面上点。

---

## 6. 三个典型流程

### 6.1 「把那个确定按钮点掉」

```
list_windows → focus_window(title="某某软件")
→ find_text(keyword="确定")            # 拿到 center_x / center_y
→ click_at(x=…, y=…)                   # 坐标直接用
```

不用先截图：`find_text` 不给图就截屏，坐标已经是屏幕绝对坐标。

### 6.2 「这张表读成 Excel 能粘的样子」

```
screenshot(image_path=…) 或 read_table(image_path="C:/x.png")
```

`read_table` 直接给三份结果：`html`（带合并单元格）、`text`（Tab 分隔，粘 Excel）、
`cells`（每个格子的坐标文字）。`truncated:true` 表示表太大被截断了，别拿半张表当真。

### 6.3 「帮我看看这个软件报了什么错」

```
read_table / ocr_image(source="window", title="某某")   # 只认这个窗口，不认全屏
```

截窗口走 `PrintWindow`，窗口被别的窗口挡住也能抓。

---

## 7. 实现要点（给维护者）

- 工具表 `src-tauri/src/api/ai_tools.rs` 的 `TOOLS` 是**唯一清单**：`catalog()`、
  `risk_of()`、界面上的工具数都从它来。**加工具必须同时在 `exec` 里加分支**，
  否则会走到 `other` 并明确报「清单里有但没实现（程序 bug）」。
- 参数解析刻意宽松（`"0.3"` 这种字符串数字也收），区间用 clamp 夹住；
  必填参数缺失时**必须点名缺哪个**，AI 才能自我修正。
- 键鼠注入在 `src-tauri/src/input.rs`：`SendInput` + 虚拟屏绝对坐标归一化
  （`MOUSEEVENTF_VIRTUALDESK`，按「虚拟屏宽高 - 1」映射到 0..65535）。
  用主屏尺寸归一化的话，副屏在主屏左侧时点下去会整体偏移一整屏。
- 输入只会进**前台窗口**；`SetForegroundWindow` 受前台锁限制会静默失败，
  所以 `focus_window` 如实返回 `foreground:false`，不假装成功。内部会依次尝试：
① 发一次空 ALT 键（Windows 把它当作用户有焦点变更意图）；② AttachThreadInput 把自己的
输入线程贴到当前前台窗口的线程上，借它的身份 SetForegroundWindow；③ 再读一次
GetForegroundWindow 复核（这个 API 有时成功了也返回 false）。三道都不剩才会报 false。
- 每一个底层能力都尽量只写一遍：`ocr_image`、`find_text`、`read_table`、
  `find_target` 共用 `load_image()`（本地图 / 截屏 / 截窗口 + `region` 裁剪），
  坐标平移只在这一处做。

---

## 7.5 已知限制（实测，2026-10-24）

这几条都是端到端验收时真实撞上的，不是推测：

1. **OCR 认不出「白字彩底」按钮。** 界面里的主按钮（蓝底白字「立即更新」这类）
   PP-OCR small 连检测框都出不来，`find_text` 会报告「没找到」，而按钮就在屏幕上。
   绕法：截图后按颜色聚类找到那坨高饱和度色块（主按钮在一屏浅色界面里最扎眼），
   或者 `region` 裁出来放大后再认。`scripts/verify_upgrade.py::find_blue_button`
   就是前者的实现。
2. **小灰字（12px、neutral-500）识别率差**，容易出「新dA有工金和例以。」这种乱码。
   要找这类文字时把 `region` 裁小一点，别整屏一起认。
3. **滚轮才能看到的内容，`find_text` 默认找不到。** 先 `scroll` 再找；
   反过来，**等某个元素出现期间不要滚屏**（会把元素滚出视野，造成「明明出来了
   却搜不到」的误判）。
4. **点下去 ≠ 界面响应了。** 点击只代表事件送达。自动化里点完必须复核
   （比如等目标文字出现），不能以「返回 ok」当作成功。

---

## 8. 排错

| 现象 | 原因 |
|---|---|
| `403 AI 接口没开` | 设置 → AI 控制 的开关没开 |
| `401 连接码不正确` | 没带 `Authorization`，或带的是旧码（重置过） |
| `summary` 里的人话是乱码 | 你的终端不是 UTF-8，不影响数据 |
| `click_at` 报「系统拒绝了这次输入」 | 目标窗口是管理员权限（提权窗口收不到普通进程输入）或锁屏 |
| `focus_window` 返回 `foreground:false` | 前台锁没绕过去（极少数系统策略会拦 AttachThreadInput）。手动点一下任务栏把窗口激活，再用 `click_at` 点窗口标题栏把它带到最前 |
| `find_text` 找不到 | 先用 `ocr_image` 看看到底认出了什么；再考虑放宽关键词或调 `region` |
| 按钮就在屏幕上却找不到 | 大概率是白字彩底按钮，OCR 不出框。按颜色定位，见 §7.5 |
| `read_table` 报缺模型 | `install_model(id="table")`，或去设置 → 模型库下载 |
