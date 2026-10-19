# 格式 11 → 12 一次性迁移

此工具是本次删除来源自动同步开关的独立离线操作，不属于应用启动流程。格式 12 删除 `ingest_sources.enabled`；默认来源和用户添加的来源都直接参与自动同步。原来为 0 或 1 的来源均保留，其他功能的 enabled 设置不变。

先退出 NoEnding，并在整个操作期间保持关闭。在仓库的 `src-tauri` 目录执行：

```sh
cargo run --example migrate_v11 -- --database /绝对路径/noending.db --backup-dir /绝对路径/backups
```

默认仅备份并生成、验证候选库。检查输出后，加 `--apply` 才替换实际库：

```sh
cargo run --example migrate_v11 -- --database /绝对路径/noending.db --backup-dir /绝对路径/backups --apply
```

工具检查数据库身份与格式、独占锁、完整性和外键；通过 SQLite 备份包含 WAL 中的数据，在副本中删除这一列，逐表双向核对所有保留列与行数，并检查 FTS 索引。会话、消息 ID、消息投影、任务、项目、Context、偏好与设置均保留。不重读来源文件，不重新摄入历史。

备份以 `noending-format-11-<UUID>.db` 命名，保留原格式和来源开关值。准备或验证失败不会替换原库。工具拒绝活跃读写连接、其他数据库格式和重复迁移。应用本身继续只接受当前格式，不提供旧格式兼容分支。

恢复时先关闭 NoEnding，再用完整备份恢复数据库；格式 11 的备份需配合支持格式 11 的应用，或重新运行此工具。不要在应用运行时复制主库文件或混用其他数据库的 WAL/SHM 文件。
