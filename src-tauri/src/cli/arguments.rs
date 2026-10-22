use super::service::{Command, Kind};
use super::{CliError, CliResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) enum Invocation {
    Help(Value),
    Mcp {
        home: Option<PathBuf>,
    },
    Install {
        directory: PathBuf,
        action: String,
    },
    Request {
        home: Option<PathBuf>,
        connection: Option<PathBuf>,
        command: Value,
        request_id: Option<String>,
    },
}
struct Options(HashMap<String, Vec<String>>);
impl Options {
    fn take(&mut self, key: &str) -> Option<String> {
        self.0.remove(key).map(|mut v| v.remove(0))
    }
    fn required(&mut self, key: &str) -> CliResult<String> {
        self.take(key)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| invalid(format!("缺少 --{key}")))
    }
    fn number<T: std::str::FromStr>(&mut self, key: &str, default: T) -> CliResult<T> {
        self.take(key)
            .map(|s| s.parse().map_err(|_| invalid(format!("--{key} 无效"))))
            .unwrap_or(Ok(default))
    }
    fn finish(&self) -> CliResult<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid(format!(
                "不支持的参数：{}",
                self.0.keys().cloned().collect::<Vec<_>>().join(", ")
            )))
        }
    }
}
fn invalid(message: impl ToString) -> CliError {
    CliError::new("invalid_arguments", message)
}
fn id(words: &[String]) -> CliResult<String> {
    if words.len() != 3 || words[2].trim().is_empty() {
        return Err(invalid("需要一个目标 ID"));
    }
    Ok(words[2].clone())
}
pub(super) fn parse(args: &[String]) -> CliResult<Invocation> {
    let mut words = Vec::new();
    let mut options = Options(HashMap::new());
    let mut help = args.is_empty();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--cli" && i == 0 {
            i += 1;
            continue;
        }
        if arg == "--help" || arg == "-h" {
            help = true;
            i += 1;
            continue;
        }
        if arg == "--version" {
            return Ok(Invocation::Help(
                json!({"version":env!("CARGO_PKG_VERSION"),"schema_version":1}),
            ));
        }
        if let Some(key) = arg.strip_prefix("--") {
            let value = args
                .get(i + 1)
                .filter(|s| !s.starts_with("--"))
                .ok_or_else(|| invalid(format!("--{key} 缺少值")))?;
            if options.0.contains_key(key) && !matches!(key, "path" | "path-id") {
                return Err(invalid(format!("重复 --{key}")));
            }
            options.0.entry(key.into()).or_default().push(value.clone());
            i += 2;
        } else {
            words.push(arg.clone());
            i += 1;
        }
    }
    let connection = options.take("connection").map(PathBuf::from);
    let home = options.take("home").map(PathBuf::from);
    if help {
        return Ok(Invocation::Help(help_for(
            words.first().map(String::as_str),
        )?));
    }
    let resource = words
        .first()
        .map(String::as_str)
        .ok_or_else(|| invalid("缺少命令"))?;
    let action = words.get(1).map(String::as_str).unwrap_or("");
    if resource == "mcp" && action == "serve" && words.len() == 2 {
        if connection.is_some() {
            return Err(invalid("MCP 使用 --home，不接受 --connection"));
        }
        options.finish()?;
        return Ok(Invocation::Mcp { home });
    }
    if options.0.get("path").is_some_and(|p| p.len() > 1)
        && !(resource == "task" && action == "create")
    {
        return Err(invalid("此命令只接受一个 --path"));
    }
    if options.0.get("path-id").is_some_and(|p| p.len() > 1)
        && !(resource == "task" && action == "path" && words.get(2).is_some_and(|s| s == "reorder"))
    {
        return Err(invalid("此命令只接受一个 --path-id"));
    }
    if resource == "cli"
        && matches!(action, "install" | "status" | "update" | "uninstall")
        && words.len() == 2
    {
        let dir = PathBuf::from(options.required("dir")?);
        options.finish()?;
        return Ok(Invocation::Install {
            directory: dir,
            action: action.into(),
        });
    }
    if resource != "extraction" && connection.is_some() {
        return Err(invalid("--connection 仅用于提取查询；业务命令使用 --home"));
    }
    if resource == "extraction" && home.is_some() {
        return Err(invalid("提取查询不能使用 --home"));
    }
    let requested_id = options.take("request-id");
    let command = if resource == "extraction" {
        if words.len() != 2 {
            return Err(invalid("使用 extraction inputs/read/search"));
        }
        if connection.is_none() {
            return Err(invalid("提取命令需要 --connection"));
        }
        match action {
            "inputs" => json!({"action":"inputs"}),
            "read" | "search" => {
                let name = options.required("input")?;
                let cursor = options.number("cursor", 0usize)?;
                let limit = options.number("limit", 8usize)?;
                if !(1..=50).contains(&limit) {
                    return Err(invalid("limit 必须在 1 到 50 之间"));
                }
                let mut v = json!({"action":action,"name":name,"cursor":cursor,"limit":limit});
                if action == "search" {
                    v["query"] = json!(options.required("query")?);
                }
                v
            }
            _ => return Err(invalid("提取命令仅支持 inputs、read、search")),
        }
    } else {
        let c = match resource {
            "status" if words.len() == 1 => Command::Status,
            "freshness" if words.len() == 1 => {
                let targets = [
                    (Kind::Session, options.take("session")),
                    (Kind::Task, options.take("task")),
                    (Kind::Project, options.take("project")),
                ]
                .into_iter()
                .filter_map(|(kind, id)| id.map(|id| (kind, id)))
                .collect::<Vec<_>>();
                if targets.len() > 1 {
                    return Err(invalid("只能指定一个会话、任务或项目"));
                }
                let (kind, id) = targets
                    .into_iter()
                    .next()
                    .map(|(kind, id)| (Some(kind), Some(id)))
                    .unwrap_or((None, None));
                Command::Freshness { kind, id }
            }
            "agent" if action == "list" && words.len() == 2 => Command::Agents,
            "sync" if words.len() == 1 => {
                let source = options.take("source");
                let session = options.take("session");
                if source.is_some() && session.is_some() {
                    return Err(invalid("--source 与 --session 不能同时指定"));
                }
                Command::Sync { source, session }
            }
            "source" if action == "list" && words.len() == 2 => Command::Sources,
            "path" if action == "register" && words.len() == 2 => Command::RegisterPath {
                path: options.required("path")?,
            },
            "request" if action == "show" => Command::Receipt { id: id(&words)? },
            "search" if words.len() == 1 => Command::Search {
                query: options.required("query")?,
                project: options.take("project"),
                task: options.take("task"),
                scope: options.take("scope").unwrap_or_else(|| "unarchived".into()),
                agent: options.take("agent"),
                cursor: options.take("cursor"),
                limit: options.number("limit", 20)?,
            },
            "session" | "task" | "project" => {
                let kind = match resource {
                    "session" => Kind::Session,
                    "task" => Kind::Task,
                    _ => Kind::Project,
                };
                match action {
                    "resolve" if kind == Kind::Project && words.len() == 2 => {
                        Command::ResolveProject {
                            path: options.take("path").map(Ok).unwrap_or_else(|| {
                                std::env::current_dir().map(|p| p.to_string_lossy().into_owned())
                            })?,
                        }
                    }
                    "project" if kind == Kind::Task && words.len() == 4 => {
                        let id = words[3].clone();
                        match words[2].as_str() {
                            "list" => Command::TaskProjects { id },
                            "set" => Command::SetTaskProjects {
                                id,
                                projects: options.0.remove("project").unwrap_or_default(),
                            },
                            _ => return Err(invalid("任务项目命令无效")),
                        }
                    }
                    "update" if kind == Kind::Task => {
                        let id = id(&words)?;
                        let title = options.take("title");
                        let description = options.take("description");
                        if title.is_none() && description.is_none() {
                            return Err(invalid("需要提供 --title 或 --description"));
                        }
                        Command::UpdateTask {
                            id,
                            title,
                            description,
                        }
                    }
                    "archive" | "unarchive" => Command::Archive {
                        kind,
                        id: id(&words)?,
                        archived: action == "archive",
                    },
                    "rename" if kind == Kind::Project => Command::RenameProject {
                        id: id(&words)?,
                        name: options.required("name")?,
                    },
                    "list" if words.len() == 2 => Command::List {
                        kind,
                        project: options.take("project"),
                        task: options.take("task"),
                        scope: options.take("scope").unwrap_or_else(|| "unarchived".into()),
                        agent: options.take("agent"),
                        title: options.take("title"),
                        path: options.take("path"),
                        cursor: options.take("cursor"),
                        limit: options.number("limit", 20)?,
                    },
                    "show" => Command::Show {
                        kind,
                        id: id(&words)?,
                    },
                    "summary" => Command::Summary {
                        kind,
                        id: id(&words)?,
                        item: options
                            .take("item")
                            .map(|s| s.parse().map_err(|_| invalid("--item 无效")))
                            .transpose()?,
                        revision: options
                            .take("revision")
                            .map(|s| s.parse().map_err(|_| invalid("--revision 无效")))
                            .transpose()?,
                    },
                    "messages" | "search" if kind == Kind::Session => Command::Messages {
                        id: id(&words)?,
                        cursor: options.take("cursor"),
                        query: if action == "search" {
                            Some(options.required("query")?)
                        } else {
                            None
                        },
                        from: options.number("from", 1)?,
                        to: options
                            .take("to")
                            .map(|s| s.parse().map_err(|_| invalid("--to 无效")))
                            .transpose()?,
                        limit: options.number("limit", 8)?,
                    },
                    "create" if kind == Kind::Task && words.len() == 2 => Command::CreateTask {
                        title: options.required("title")?,
                        description: options.take("description").unwrap_or_default(),
                        projects: options.0.remove("project").unwrap_or_default(),
                    },
                    "attach" if kind == Kind::Task => Command::Attach {
                        task: id(&words)?,
                        session: options.required("session")?,
                    },
                    "detach" if kind == Kind::Task => Command::Detach {
                        task: id(&words)?,
                        session: options.required("session")?,
                    },
                    _ => return Err(invalid("命令无效，使用 --help 查看用法")),
                }
            }
            "summary" if action == "config" && words.len() == 3 => match words[2].as_str() {
                "show" => Command::ExtractionSettings,
                "set" => {
                    let patch = super::extraction_settings::Patch {
                        agent: options.take("agent"),
                        model: options.take("model"),
                        provider: options.take("provider"),
                        effort: options.take("effort"),
                    };
                    if patch.is_empty() {
                        return Err(invalid("至少提供一个提取配置字段"));
                    }
                    Command::ConfigureExtraction { patch }
                }
                _ => return Err(invalid("使用 summary config show/set")),
            },
            "summary" if action == "search" && words.len() == 2 => Command::SearchSummaries {
                query: options.required("query")?,
                kind: options.take("kind"),
                project: options.take("project"),
                scope: options.take("scope").unwrap_or_else(|| "unarchived".into()),
                cursor: options.take("cursor"),
                limit: options.number("limit", 20)?,
            },
            "summary" => {
                if words.len() != 4 {
                    return Err(invalid(
                        "使用 summary update/regenerate/set <session|task|project> <id>，或 summary lock/unlock task <id>",
                    ));
                }
                let kind: Kind = serde_json::from_value(json!(words[2]))
                    .map_err(|_| invalid("摘要目标类型无效"))?;
                let id = words[3].clone();
                match action {
                    "update" | "regenerate" => Command::Extract {
                        kind,
                        id,
                        regenerate: action == "regenerate",
                    },
                    "set" => Command::EditSummary {
                        kind,
                        id,
                        revision: options.number("revision", -1)?,
                        content: std::fs::read_to_string(options.required("file")?)?,
                    },
                    "lock" | "unlock" => {
                        if kind != Kind::Task {
                            return Err(invalid("仅任务摘要支持通过 CLI 加锁或解锁"));
                        }
                        Command::SetTaskSummaryTitleLock {
                            id,
                            revision: options.number("revision", -1)?,
                            item: options.number("item", 0)?,
                            locked: action == "lock",
                        }
                    }
                    _ => return Err(invalid("摘要命令无效")),
                }
            }
            _ => return Err(invalid("未知命令，使用 ne --help 查看用法")),
        };
        serde_json::to_value(c)?
    };
    let (command, request_id) = if resource != "extraction"
        && serde_json::from_value::<Command>(command.clone())?.is_write()
    {
        let id = requested_id.unwrap_or_else(crate::storage::new_id);
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(invalid("请求编号需为 1～128 个字母、数字、短横线或下划线"));
        }
        (
            json!({"action":"execute_once","request_id":id,"command":command}),
            Some(id),
        )
    } else {
        if requested_id.is_some() {
            return Err(invalid("--request-id 只支持写操作"));
        }
        (command, None)
    };
    options.finish()?;
    Ok(Invocation::Request {
        home,
        connection,
        command,
        request_id,
    })
}
fn help_for(resource: Option<&str>) -> CliResult<Value> {
    let usage = match resource {
        None => vec!["ne status", "ne freshness --help", "ne mcp serve [--home ABSOLUTE_DIRECTORY]", "ne agent list", "ne sync --help", "ne source list", "ne search --help", "ne session/task/project --help", "ne summary --help", "ne path --help", "ne request --help", "ne cli install --dir <PATH目录>"],
        Some("mcp") => vec!["ne mcp serve [--home ABSOLUTE_DIRECTORY]"],
        Some("session") => vec!["ne session list [--project ID] [--task ID] [--agent AGENT] [--scope unarchived|archived|all] [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne session archive ID", "ne session unarchive ID", "ne session show ID", "ne session summary ID [--item NUMBER --revision REVISION]", "ne session messages ID [--from ORDINAL] [--to ORDINAL] [--cursor CURSOR] [--limit 8]", "ne session search ID --query TEXT [--from ORDINAL] [--to ORDINAL] [--cursor CURSOR] [--limit 8]"],
        Some("task") => vec!["ne task list [--project ID] [--scope unarchived|archived|all] [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne task show ID", "ne task summary ID [--item NUMBER --revision REVISION]", "ne task create --title TEXT [--description TEXT] [--project ID ...]", "ne task update ID [--title TEXT] [--description TEXT]", "ne task archive ID", "ne task unarchive ID", "ne task project list ID", "ne task project set ID [--project ID ...]", "ne task attach ID --session ID", "ne task detach ID --session ID"],
        Some("project") => vec!["ne project resolve [--path ABSOLUTE_DIRECTORY]", "ne project archive ID", "ne project unarchive ID", "ne project rename ID --name TEXT", "ne project list [--scope unarchived|archived|all] [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne project show ID", "ne project summary ID [--item NUMBER --revision REVISION]"],
        Some("summary") => vec!["ne summary config show", "ne summary config set [--agent codex|claude_code|pi|none] [--model TEXT] [--provider TEXT] [--effort TEXT]", "ne summary search --query TEXT [--kind session|task|project] [--project ID] [--scope unarchived|archived|all] [--cursor CURSOR] [--limit 20]","ne summary update session|task|project ID", "ne summary regenerate session|task|project ID", "ne summary set session|task|project ID --file FILE --revision REVISION", "ne summary lock task ID --item NUMBER --revision REVISION", "ne summary unlock task ID --item NUMBER --revision REVISION"],
        Some("sync") => vec!["ne sync [--source SOURCE_ID | --session SESSION_ID]"],
        Some("agent") => vec!["ne agent list"],
        Some("source") => vec!["ne source list"],
        Some("search")=>vec!["ne search --query TEXT [--project ID] [--task ID] [--agent AGENT] [--scope unarchived|archived|all] [--cursor CURSOR] [--limit 20]"],
        Some("request")=>vec!["ne request show ID", "写操作可用 --request-id ID 重试相同操作；查询操作不接受该选项"],
        Some("path")=>vec!["ne path register --path ABSOLUTE_EXISTING_DIRECTORY"],
        Some("freshness")=>vec!["ne freshness [--session ID | --task ID | --project ID]"],
        Some("status")=>vec!["ne status"],
        Some("cli")=>vec!["ne cli install --dir <PATH目录>", "ne cli status --dir <PATH目录>", "ne cli update --dir <PATH目录>", "ne cli uninstall --dir <PATH目录>"],
        Some("extraction")=>vec!["ne --connection FILE extraction inputs", "ne --connection FILE extraction read --input NAME [--cursor 0] [--limit 8]", "ne --connection FILE extraction search --input NAME --query TEXT [--cursor 0] [--limit 8]"],
        _=>return Err(invalid("未知命令")),
    };
    Ok(
        json!({"usage":usage,"output":"JSON; schema_version=1; ok=false 时退出码非零","home":"独立访问当前 NoEnding Home；--home 指定数据目录，无需运行桌面应用。--connection 仅用于提取查询。","messages":"按会话 ordinal 排序；长消息分为连续 part，完整拼接后阅读；分页不包含开始查询后追加的消息。搜索是大小写敏感子串匹配。","summary":"update/regenerate 等待提取及入库完成后返回最终结果；失败返回非零退出码。set 的 FILE 必须是 {items:[{title,description,details,title_locked}]} JSON，并提供当前摘要版本，保留消息处理进度。set 必须保留锁定标题和锁定状态，描述和细则可更新；lock/unlock 仅显式切换任务摘要的一个标题锁，需要 --item 与 --revision，返回新版本和完整事项。摘要读取返回事项列表；按事项读取需同时提供 --item 与 --revision。"}),
    )
}
