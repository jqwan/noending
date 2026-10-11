use super::service::{Command, Kind};
use super::{CliError, CliResult};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

pub(super) enum Invocation {
    Help(Value),
    Install {
        directory: PathBuf,
        action: String,
    },
    Request {
        connection: Option<PathBuf>,
        command: Value,
        wait: Option<std::time::Duration>,
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
            if key == "wait" {
                if options.0.insert(key.into(), vec!["true".into()]).is_some() {
                    return Err(invalid("重复 --wait"));
                }
                i += 1;
                continue;
            }
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
    let wait = if (resource == "summary"
        && matches!(action, "update" | "regenerate")
        && options.take("wait").is_some())
        || (resource == "job" && action == "wait")
    {
        let seconds = options.number("timeout", 600u64)?;
        if !(1..=86400).contains(&seconds) {
            return Err(invalid("--timeout 必须在 1 到 86400 秒之间"));
        }
        Some(std::time::Duration::from_secs(seconds))
    } else {
        None
    };
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
                    "path"
                        if kind == Kind::Task
                            && words.len() == 4
                            && !words[3].trim().is_empty() =>
                    {
                        let id = words[3].clone();
                        match words[2].as_str() {
                            "list" => Command::TaskPaths { id },
                            "add" => Command::AddTaskPath {
                                id,
                                path: options.required("path")?,
                            },
                            "remove" => Command::RemoveTaskPath {
                                id,
                                path_id: options.required("path-id")?,
                            },
                            "reorder" => Command::ReorderTaskPaths {
                                id,
                                path_ids: options.0.remove("path-id").unwrap_or_default(),
                            },
                            _ => return Err(invalid("任务目录命令无效")),
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
                    "archive" | "unarchive" if kind != Kind::Project => Command::Archive {
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
                        paths: options.0.remove("path").unwrap_or_default(),
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
            "summary" => {
                if words.len() != 4 {
                    return Err(invalid(
                        "使用 summary update/regenerate/set <session|task|project> <id>",
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
                    _ => return Err(invalid("摘要命令无效")),
                }
            }
            "preference" => match action {
                "list" | "add" if words.len() == 2 => {
                    let scope = options.required("scope")?;
                    let project = options.take("project");
                    if !matches!(scope.as_str(), "user" | "project")
                        || (scope == "project") != project.is_some()
                    {
                        return Err(invalid("user 不提供 --project；project 必须提供 --project"));
                    }
                    if action == "list" {
                        Command::Preferences { project }
                    } else {
                        Command::AddPreference {
                            project,
                            content: options.required("content")?,
                        }
                    }
                }
                "update" => Command::UpdatePreference {
                    id: id(&words)?,
                    content: options.required("content")?,
                },
                "delete" => Command::DeletePreference { id: id(&words)? },
                _ => return Err(invalid("偏好命令无效")),
            },
            "job" => match action {
                "list" if words.len() == 2 => Command::Jobs,
                "show" | "wait" => Command::Job { id: id(&words)? },
                _ => return Err(invalid("作业命令无效")),
            },
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
        connection,
        command,
        wait,
        request_id,
    })
}
fn help_for(resource: Option<&str>) -> CliResult<Value> {
    let usage = match resource {
        None => vec!["ne status", "ne search --help", "ne session/task/project --help", "ne preference --help", "ne summary --help", "ne job --help", "ne path --help", "ne request --help", "ne cli install --dir <PATH目录>"],
        Some("session") => vec!["ne session list [--project ID] [--task ID] [--agent AGENT] [--scope unarchived|archived|all] [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne session archive ID", "ne session unarchive ID", "ne session show ID", "ne session summary ID", "ne session messages ID [--from ORDINAL] [--to ORDINAL] [--cursor CURSOR] [--limit 8]", "ne session search ID --query TEXT [--from ORDINAL] [--to ORDINAL] [--cursor CURSOR] [--limit 8]"],
        Some("task") => vec!["ne task list [--project ID] [--scope unarchived|archived|all] [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne task show ID", "ne task summary ID", "ne task create --title TEXT [--description TEXT] [--path PATH ...]", "ne task update ID [--title TEXT] [--description TEXT]", "ne task archive ID", "ne task unarchive ID", "ne task path list ID", "ne task path add ID --path PATH", "ne task path remove ID --path-id WORKSPACE_PATH_ID", "ne task path reorder ID [--path-id WORKSPACE_PATH_ID ...]", "ne task attach ID --session ID", "ne task detach ID --session ID"],
        Some("project") => vec!["ne project rename ID --name TEXT", "ne project list [--title TEXT] [--path PATH] [--cursor CURSOR] [--limit 20]", "ne project show ID", "ne project summary ID"],
        Some("summary") => vec!["ne summary update session|task|project ID [--wait] [--timeout 600]", "ne summary regenerate session|task|project ID [--wait] [--timeout 600]", "ne summary set session|task|project ID --file FILE --revision REVISION"],
        Some("preference") => vec!["ne preference list --scope user|project [--project ID]", "ne preference add --scope user|project [--project ID] --content TEXT", "ne preference update ID --content TEXT", "ne preference delete ID"],
        Some("job") => vec!["ne job list", "ne job show ID", "ne job wait ID [--timeout 600]"],
        Some("search")=>vec!["ne search --query TEXT [--project ID] [--task ID] [--agent AGENT] [--scope unarchived|archived|all] [--cursor CURSOR] [--limit 20]"],
        Some("request")=>vec!["ne request show ID", "写操作可用 --request-id ID 重试相同操作；查询操作不接受该选项"],
        Some("path")=>vec!["ne path register --path ABSOLUTE_EXISTING_DIRECTORY"],
        Some("status")=>vec!["ne status"],
        Some("cli")=>vec!["ne cli install --dir <PATH目录>", "ne cli status --dir <PATH目录>", "ne cli update --dir <PATH目录>", "ne cli uninstall --dir <PATH目录>"],
        Some("extraction")=>vec!["ne --connection FILE extraction inputs", "ne --connection FILE extraction read --input NAME [--cursor 0] [--limit 8]", "ne --connection FILE extraction search --input NAME --query TEXT [--cursor 0] [--limit 8]"],
        _=>return Err(invalid("未知命令")),
    };
    Ok(
        json!({"usage":usage,"output":"JSON; schema_version=1; ok=false 时退出码非零","connection":"默认连接正在运行的 NoEnding；--connection 指定连接。提取连接只允许 extraction 查询。","messages":"按会话 ordinal 排序；长消息分为连续 part，完整拼接后阅读；分页不包含开始查询后追加的消息。搜索是大小写敏感子串匹配。","summary_jobs":"update/regenerate 返回 job_id，通过 job show/wait 查询后端确认的最终结果；--wait 等待完成，失败返回非零退出码，等待超时保留作业。set 必须提供当前摘要版本，保留消息处理进度。"}),
    )
}
