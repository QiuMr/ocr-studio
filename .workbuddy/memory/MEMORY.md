# OCR-Studio 项目长期记忆

## 前端技术规范（2026-10-21 用户确定，必须遵守）
- 技术栈：**React + TypeScript + Tailwind CSS + shadcn/ui**（Vite，Tauri 2 壳不变）。
  原 Vue 3 + shadcn-vue 方案已废弃，`src/` 下 Vue 骨架需整体替换。
- 字体：**MiSans**（小米），`@font-face` 本地引入并设为全局默认；商用授权需确认后再随包分发。
- 配色：**纯白为主，黑色为辅，蓝色点缀**。
- **每个按钮都必须带图标**（用 `lucide-react`，shadcn/ui 默认图标源）。
- **自定义标题栏：不要图标、不要标题**，仅窗口控制按钮。
  `tauri.conf.json` 已设 `decorations: false`，前端用 `getCurrentWindow()` 自绘。

## 项目环境约定
- Rust 工具链是**项目内私有安装**：跑 cargo 前必须设
  `RUSTUP_HOME=D:\Python测试\OCR-Studio\.rust\rustup`、`CARGO_HOME=D:\Python测试\OCR-Studio\.rust\cargo`。
- Bash 工具在本机 exit 127（MSYS coreutils 版本不匹配），**全程用 PowerShell 工具**。
- cv2 读不了中文路径，一律 `np.frombuffer + cv2.imdecode`。
- 写 `.ps1` 必须 UTF-8 带 BOM + CRLF（PowerShell 5.1 按 GBK 解码无 BOM 文件）。

## 精度基线（改动前后处理前必读）
- 黄金基线：`models/small/reference_output.json`，由 `scripts/ref_pipeline.py` 生成。
- 验收命令：`cargo test -- --ignored`（37 个测试，含 pipeline_match 全流程比对）。
- **必须复刻的三个底层语义**（详见 `docs/PROGRESS.md` 第 3 节）：
  1. `cv2.resize`：横向 11bit 定点 + 纵向截断的两段式实现；
  2. `cv2.warpPerspective`：整数坐标=像素中心、源坐标量化到 1/32、越界逐 tap 补 0；
  3. `pyclipper`：输入坐标截断成整数、距离保持 double、输出 half-up 四舍五入。
- **误检框的置信度是混沌的**，1-LSB 扰动能抖 0.04，**不能用误检框做精度验收**。
- 验收时必须打**全精度**框坐标：整数取整后会掩盖亚像素差异（本轮就因此漏判过）。

## 当前进度（2026-10-21）
- Rust 推理核心完成并与 Python 基线逐位对齐；HTTP 接口层、Tauri 命令层、前端未开始。
- 详细记录见 `docs/PROGRESS.md`。
