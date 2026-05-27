# sync-folder

将文件夹 A 的内容同步到文件夹 B 的 Rust 命令行工具。

## 构建

```bash
cargo build --release
# 产物：target/release/sync-folder
```

## 测试

```bash
bash test.sh
```

或使用 `/test` slash command 触发 Claude 自动运行测试。

## 项目结构

- `src/main.rs` — 主程序逻辑
- `test.sh` — 集成测试脚本（10 个场景，21 个断言）
- `.claude/commands/test.md` — `/test` slash command 定义

## 核心逻辑

四阶段：**扫描 → 比对 → 移动识别 → 展示计划 → 用户确认 → 执行**

文件相同判断：文件名 + 文件大小（不比较内容）。

删除操作：移到 `B/_trash_YYYYMMDD_HHMMSS/`，不实际删除。
