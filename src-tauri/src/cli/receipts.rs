//! Request receipts survive individual CLI invocations. A crash after a write
//! but before its receipt is complete is ambiguous, never permission to replay.
use super::{transport, CliError, CliResult};
use crate::{platform::file_lock, workspace::home::NoEndingHome};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    command_hash: String,
    result: Option<CliResult<Value>>,
}
fn path(home: &NoEndingHome, id: &str) -> CliResult<PathBuf> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(CliError::new(
            "invalid_arguments",
            "请求编号需为 1～128 个字母、数字、短横线或下划线",
        ));
    }
    Ok(home
        .runtime_dir
        .join("cli-requests")
        .join(format!("{:x}.json", Sha256::digest(id.as_bytes()))))
}
fn read(file: &mut File) -> CliResult<Receipt> {
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    serde_json::from_str(&text).map_err(|_| {
        CliError::new(
            "request_interrupted",
            "请求记录不完整，请核对业务状态，不要直接重复写入",
        )
    })
}
fn save(file: &mut File, receipt: &Receipt) -> CliResult<()> {
    file.seek(SeekFrom::Start(0))?;
    let bytes = serde_json::to_vec(receipt)?;
    file.write_all(&bytes)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()?;
    Ok(())
}
pub(super) fn show(home: &NoEndingHome, id: &str) -> CliResult<Value> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path(home, id)?)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                CliError::new("not_found", "请求记录不存在")
            } else {
                error.into()
            }
        })?;
    match file_lock::try_lock(&file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            return Ok(json!({"request_id":id,"status":"running"}))
        }
        Err(error) => return Err(error.into()),
    }
    let receipt = read(&mut file)?;
    Ok(
        json!({"request_id":id,"status":if receipt.result.is_some(){"completed"}else{"interrupted"},"response":receipt.result.map(super::envelope)}),
    )
}
pub(super) fn execute(
    home: &NoEndingHome,
    id: &str,
    command: &[u8],
    run: impl FnOnce() -> CliResult<Value>,
) -> CliResult<Value> {
    let path = path(home, id)?;
    transport::private_dir(path.parent().unwrap())?;
    let mut file = file_lock::exclusive(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            CliError::new("request_running", "请求正在执行，请查询 request show")
        } else {
            error.into()
        }
    })?;
    let command_hash = format!("{:x}", Sha256::digest(command));
    if file.metadata()?.len() > 0 {
        let receipt = read(&mut file)?;
        if receipt.command_hash != command_hash {
            return Err(CliError::new(
                "request_conflict",
                "此请求编号已用于其他操作",
            ));
        }
        return receipt.result.unwrap_or_else(|| {
            Err(CliError::new(
                "request_interrupted",
                "上次请求未确认完成，请核对业务状态，不要直接重复写入",
            ))
        });
    }
    let mut receipt = Receipt {
        command_hash,
        result: None,
    };
    save(&mut file, &receipt)?;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).unwrap_or_else(|_| {
        Err(CliError::new(
            "internal_error",
            "操作异常终止，请查询业务状态",
        ))
    });
    receipt.result = Some(result.clone());
    save(&mut file, &receipt)?;
    result
}
