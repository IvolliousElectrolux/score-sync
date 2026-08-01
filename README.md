# Score Sync / 曲谱同步

方便曲谱同步一条龙制作的 Rust GPUI 工具.

从扫描谱或 PDF 出发, 在同一界面完成 **谱表分块 → 蒙版清理 → 加底色裁切**, 并支持工程保存/继续编辑, 方便后续做成同步曲谱视频.

## 功能

| 面板 | 作用 |
|------|------|
| **分块** | 自动识别大谱表行, 跨页组合, 导出竖向拼接切片 |
| **蒙版** | 在组合拼合图上框选半透明白蒙版, 遮挡不想出现的记号 |
| **工程** | 打开/保存 `.staffcrop` 工程包; 内嵌「谱面加底色」批量处理 |

## 环境

- Windows (当前以 Windows + GPUI 为主)
- Rust stable (`cargo` / `rustc`)
- PDF 打开依赖随仓库附带的 `assets/pdfium.dll`

## 构建与运行

```bash
cd score_sync
cargo run -r
```

也可传入初始文件:

```bash
cargo run -r -- path\to\page.png path\to\score.pdf
cargo run -r -- path\to\project.staffcrop
```

发布构建产物: `target/release/score_sync.exe`.

## 工程格式

- 扩展名: `.staffcrop` (zip)
- 内含 `project.json` 与 `pages/*.png`
- 快捷键: `Ctrl+S` 保存, `Ctrl+Shift+S` 另存, `Ctrl+Shift+O` 打开

## 仓库结构

```
score_sync/           # 主程序 (本仓库根)
  src/                # 分块 / 工程 / 检测 / GUI
  assets/             # 图标, pdfium.dll
  crates/
    mask_tool/        # 蒙版库 (可嵌入)
    apply_bg/         # 谱面加底色库 (可嵌入)
```

## 常用快捷键

应用内按 `H` / `F1` 可查看完整说明. 概要:

- `Ctrl+O` 打开图片/PDF
- `D` / `A` 识别本页 / 全部页
- `M` 合并选中块 · `E` 导出组合
- 右侧切到蒙版后: 拖拽绘制, `Ctrl+Z`/`Y` 撤重

## License

MIT
