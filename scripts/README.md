# 数据库 v3 一次性转换

`migrate-database-v3.py` 是显式执行的离线工具，用于将已经扁平化的
NoEnding v1/v2 数据库转换到本次精简后的 v3。它不参与应用启动，应用仍只
支持 `src-tauri/src/storage/schema.rs` 定义的当前格式。后续格式变动应按
项目约定重建，此工具不会自动跟随新版本工作。

先完全退出 NoEnding，保持应用关闭直到转换结束。使用带 FTS5 的 Python 3：

```sh
# 备份并生成经校验的新库，保留原库不替换
python3 scripts/migrate-database-v3.py --database /absolute/path/noending.db

# 备份、转换、校验后替换原库
python3 scripts/migrate-database-v3.py --database /absolute/path/noending.db --apply
```

工具通过 SQLite Backup API 备份，包括已提交的 WAL 数据；从唯一 schema
源创建新库，逐表比较所有保留字段和转换字段，重建搜索索引，检查外键和
完整性。v1 的任务回收站状态转换为已归档，会话 `trashed_at` 转为
`archived_at`；任务进行中/已完成标记按新产品逻辑移除。消息历史、摄入游标、
Context 修订与引用、任务归属、目录关联和本地设置原样保留。

备份以 `noending.db.backup-<UTC时间>-<随机后缀>` 命名，旁边保存 JSON 校验
报告。遇到未知表/字段、非空项目描述、异常归档状态、外键损坏、数据库被
占用或备份后发生写入时，拒绝替换。工具不访问 Agent 源文件。

替换后使用支持 v3 的当前代码启动 NoEnding。若需回退，先关闭应用，再用
保留的备份恢复数据库，并配合原数据库格式对应的应用版本使用；不能把
旧库仅改版本号后交给新版应用。

验证工具：

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts -p 'test_migrate_database_v3.py' -v
```
