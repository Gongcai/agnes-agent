//! Provider-independent cloud storage tools for the agent.
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

use crate::db::repo::storage::StorageTransferJobRow;
use crate::error::{AppError, AppResult};
use crate::storage::service::StorageAccountView;
use crate::tools::builtin::{BuiltinTool, ToolCtx};
use crate::tools::policy::Risk;

const MAX_PAGE_SIZE: u64 = 200;

async fn fail<T>(ctx: &ToolCtx<'_>, message: impl std::fmt::Display) -> AppResult<T> {
    let message = message.to_string();
    ctx.record_failure(&message).await?;
    Err(AppError::Other(message))
}

fn required_string(args: &Value, name: &str) -> Result<String, AppError> {
    let value = args
        .get(name)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Other(format!("`{name}` must be a non-empty string")))?;
    Ok(value.to_string())
}

fn optional_string(args: &Value, name: &str) -> Result<Option<String>, AppError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            Ok((!value.trim().is_empty()).then(|| value.trim().to_string()))
        }
        _ => Err(AppError::Other(format!(
            "`{name}` must be a string or null"
        ))),
    }
}

fn optional_u64(args: &Value, name: &str) -> Result<Option<u64>, AppError> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(value)) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| AppError::Other(format!("`{name}` must be a non-negative integer"))),
        _ => Err(AppError::Other(format!(
            "`{name}` must be an integer or null"
        ))),
    }
}

fn parse_file_ids(args: &Value) -> Result<Vec<String>, AppError> {
    let values = args
        .get("file_ids")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::Other("`file_ids` must be an array".into()))?;
    if values.is_empty() || values.len() > 100 {
        return Err(AppError::Other(
            "`file_ids` must contain between 1 and 100 items".into(),
        ));
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let file_id = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    AppError::Other(format!("`file_ids[{index}]` must be a non-empty string"))
                })?;
            Ok(file_id.to_string())
        })
        .collect()
}

fn page_size(args: &Value) -> usize {
    args.get("page_size")
        .and_then(Value::as_u64)
        .unwrap_or(50)
        .clamp(1, MAX_PAGE_SIZE) as usize
}

fn check_enabled(ctx: &ToolCtx<'_>) -> Result<(), String> {
    if !ctx.policy.storage.enabled {
        return Err("Storage tools are disabled for this agent".into());
    }
    if !ctx.policy.network.allow {
        return Err("Network access is disabled for this agent".into());
    }
    Ok(())
}

fn check_account(ctx: &ToolCtx<'_>, account_id: &str) -> Result<(), String> {
    check_enabled(ctx)?;
    ctx.policy.check_storage(account_id)
}

fn workspace_path(ctx: &ToolCtx<'_>, raw: &str) -> Result<PathBuf, AppError> {
    let raw_path = Path::new(raw.trim());
    if raw_path.as_os_str().is_empty() || raw_path.is_absolute() {
        return Err(AppError::Other(
            "Storage paths must be relative to the workspace".into(),
        ));
    }
    if raw_path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(AppError::Other(
            "Storage paths must not contain `..`".into(),
        ));
    }
    let root = ctx.workspace_cwd.clone().ok_or_else(|| {
        AppError::Other("No workspace is available for this storage operation".into())
    })?;
    let root = crate::tools::builtin::normalize_path(&root);
    let path = crate::tools::builtin::normalize_path(&root.join(raw_path));
    if !path.starts_with(&root) {
        return Err(AppError::Other("Storage path escapes the workspace".into()));
    }
    Ok(path)
}

fn account_json(account: &StorageAccountView) -> Value {
    let capabilities = serde_json::from_str::<Value>(&account.account.capabilities_json)
        .unwrap_or_else(|_| json!({}));
    let capability = |name: &str| {
        capabilities
            .get(name)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    json!({
        "account_id": account.account.id,
        "provider_id": account.account.provider_id,
        "display_name": account.account.display_name,
        "auth_state": account.account.auth_state,
        "enabled": account.account.enabled,
        "provider_installed": account.provider_installed,
        "capabilities": {
            "browse": capability("browse_files"),
            "search": capability("search_files"),
            "download": capability("read_files"),
            "upload": capability("write_files"),
            "move": capability("move_files"),
            "trash": capability("delete_files"),
            "quota": capability("quota")
        }
    })
}

async fn transfer_result(
    ctx: &ToolCtx<'_>,
    account_id: &str,
    transfer_id: &str,
) -> AppResult<Value> {
    let jobs = ctx
        .storage
        .list_transfers(Some(account_id.to_string()), 200)
        .await?;
    let job = jobs
        .into_iter()
        .find(|job| job.id == transfer_id)
        .ok_or_else(|| {
            AppError::Other("Storage transfer completed but its journal entry was not found".into())
        })?;
    Ok(transfer_json(&job))
}

fn transfer_json(job: &StorageTransferJobRow) -> Value {
    let mut value = serde_json::to_value(job).unwrap_or_else(|_| json!({}));
    if job.destination_kind.as_deref() == Some("local_file") {
        if let Some(object) = value.as_object_mut() {
            object.insert("destination_id".into(), Value::Null);
        }
    }
    value
}

fn transfer_schema(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": {
                "type": "object",
                "properties": properties,
                "required": required,
                "additionalProperties": false
            }
        }
    })
}

pub struct StorageAccountsListTool;

#[async_trait]
impl BuiltinTool for StorageAccountsListTool {
    fn name(&self) -> &'static str {
        "storage_accounts_list"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "List connected cloud storage accounts and their capabilities. Credentials and private configuration are never returned.", json!({}), &[])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if !ctx.policy.storage.enabled {
            return fail(ctx, "Storage tools are disabled for this agent").await;
        }
        let accounts = ctx.storage.list_accounts().await?;
        let accounts = accounts.iter().map(account_json).collect::<Vec<_>>();
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Listed {} storage accounts", accounts.len())),
            None,
        )
        .await?;
        Ok(json!({"accounts": accounts}))
    }
}

pub struct StorageFilesListTool;

#[async_trait]
impl BuiltinTool for StorageFilesListTool {
    fn name(&self) -> &'static str {
        "storage_files_list"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "List remote files and folders in a connected cloud storage account. Returns metadata only.", json!({"account_id":{"type":"string"},"parent_id":{"type":["string","null"]},"page_token":{"type":["string","null"]},"page_size":{"type":"integer","minimum":1,"maximum":200}}), &["account_id"])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let args = ctx.args;
        let account_id = match required_string(args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let parent_id = match optional_string(args, "parent_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let page_token = match optional_string(args, "page_token") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let page = match ctx
            .storage
            .list_files(
                account_id.clone(),
                crate::storage::ListFilesRequest {
                    parent_id,
                    page_token,
                    page_size: page_size(args),
                },
            )
            .await
        {
            Ok(page) => page,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Listed {} remote items", page.items.len())),
            None,
        )
        .await?;
        Ok(
            json!({"account_id": account_id, "items": page.items, "next_page_token": page.next_page_token}),
        )
    }
}

pub struct StorageFilesSearchTool;

#[async_trait]
impl BuiltinTool for StorageFilesSearchTool {
    fn name(&self) -> &'static str {
        "storage_files_search"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "Search remote file names and provider-supported metadata. This is not guaranteed to be full-text content search.", json!({"account_id":{"type":"string"},"query":{"type":"string","minLength":1,"maxLength":200},"page_token":{"type":["string","null"]},"page_size":{"type":"integer","minimum":1,"maximum":200}}), &["account_id","query"])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let query = match required_string(ctx.args, "query") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let page_token = match optional_string(ctx.args, "page_token") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let page = match ctx
            .storage
            .search_files(
                account_id.clone(),
                crate::storage::SearchFilesRequest {
                    query,
                    page_token,
                    page_size: page_size(ctx.args),
                },
            )
            .await
        {
            Ok(page) => page,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Found {} remote items", page.items.len())),
            None,
        )
        .await?;
        Ok(
            json!({"account_id": account_id, "items": page.items, "next_page_token": page.next_page_token}),
        )
    }
}

pub struct StorageFileDownloadTool;

#[async_trait]
impl BuiltinTool for StorageFileDownloadTool {
    fn name(&self) -> &'static str {
        "storage_file_download"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "Download one remote file into the current workspace. The destination must be relative to the workspace.", json!({"account_id":{"type":"string"},"file_id":{"type":"string"},"destination_path":{"type":"string"},"expected_revision":{"type":["string","null"]},"expected_size":{"type":["integer","null"],"minimum":0}}), &["account_id","file_id","destination_path"])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Medium
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let file_id = match required_string(ctx.args, "file_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let destination_arg = match required_string(ctx.args, "destination_path") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let destination = match workspace_path(ctx, &destination_arg) {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let expected_revision = match optional_string(ctx.args, "expected_revision") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let expected_size = match optional_u64(ctx.args, "expected_size") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        // Provider detail endpoints can disagree with list results about file type and
        // downloadability. Treat those fields as hints and let the download endpoint decide.
        if expected_size.is_some_and(|size| size > ctx.policy.storage.max_download_bytes) {
            return fail(ctx, "Remote file exceeds the agent download size limit").await;
        }
        if destination.exists() {
            return fail(
                ctx,
                "Download destination already exists; refusing to overwrite it",
            )
            .await;
        }
        if let Err(e) = ctx.sandbox.check_write(&destination) {
            return fail(ctx, e).await;
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        ctx.update_running(&destination.to_string_lossy()).await?;
        let transfer_id = match ctx
            .storage
            .download_file_with_job_limit(
                account_id.clone(),
                file_id,
                expected_revision,
                expected_size,
                destination.to_string_lossy().to_string(),
                Some(ctx.policy.storage.max_download_bytes),
            )
            .await
        {
            Ok(transfer_id) => transfer_id,
            Err(error) => return fail(ctx, error).await,
        };
        let transfer = match transfer_result(ctx, &account_id, &transfer_id).await {
            Ok(transfer) => transfer,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some("Storage file downloaded".into()),
            None,
        )
        .await?;
        Ok(json!({"account_id": account_id, "local_path": destination_arg, "transfer": transfer}))
    }
}

pub struct StorageFileUploadTool;

#[async_trait]
impl BuiltinTool for StorageFileUploadTool {
    fn name(&self) -> &'static str {
        "storage_file_upload"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "Upload one workspace file to a connected cloud storage folder. The source path must be relative to the workspace.", json!({"account_id":{"type":"string"},"source_path":{"type":"string"},"parent_id":{"type":["string","null"]},"remote_name":{"type":["string","null"]}}), &["account_id","source_path"])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let source_arg = match required_string(ctx.args, "source_path") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let source = match workspace_path(ctx, &source_arg) {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = ctx.policy.check_file_read(&source.to_string_lossy()) {
            return fail(ctx, e).await;
        }
        if let Err(e) = ctx.sandbox.check_read(&source) {
            return fail(ctx, e).await;
        }
        let metadata = match tokio::fs::metadata(&source).await {
            Ok(metadata) => metadata,
            Err(error) => return fail(ctx, error).await,
        };
        if !metadata.is_file() {
            return fail(ctx, "Upload source must be a regular file").await;
        }
        if metadata.len() > ctx.policy.storage.max_upload_bytes {
            return fail(ctx, "Local file exceeds the agent upload size limit").await;
        }
        let parent_id = match optional_string(ctx.args, "parent_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let remote_name = match optional_string(ctx.args, "remote_name") {
            Ok(Some(v)) => v,
            Ok(None) => source.file_name().unwrap().to_string_lossy().to_string(),
            Err(e) => return fail(ctx, e).await,
        };
        ctx.update_running(&source.to_string_lossy()).await?;
        let (file, transfer_id) = match ctx
            .storage
            .upload_file_as_with_job(
                account_id.clone(),
                parent_id,
                source.to_string_lossy().to_string(),
                remote_name,
            )
            .await
        {
            Ok(result) => result,
            Err(error) => return fail(ctx, error).await,
        };
        let transfer = match transfer_result(ctx, &account_id, &transfer_id).await {
            Ok(transfer) => transfer,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some("Storage file uploaded".into()),
            None,
        )
        .await?;
        Ok(json!({"account_id": account_id, "file": file, "transfer": transfer}))
    }
}

pub struct StorageTransferStatusTool;

#[async_trait]
impl BuiltinTool for StorageTransferStatusTool {
    fn name(&self) -> &'static str {
        "storage_transfer_status"
    }
    fn schema(&self) -> Value {
        transfer_schema(
            self.name(),
            "Read the status of a cloud storage transfer started by this application.",
            json!({"transfer_id":{"type":"string"}}),
            &["transfer_id"],
        )
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        if let Err(e) = check_enabled(ctx) {
            return fail(ctx, e).await;
        }
        let transfer_id = match required_string(ctx.args, "transfer_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let jobs = match ctx.storage.list_transfers(None, 200).await {
            Ok(jobs) => jobs,
            Err(error) => return fail(ctx, error).await,
        };
        let job = match jobs.into_iter().find(|job| job.id == transfer_id) {
            Some(v) => v,
            None => return fail(ctx, "Storage transfer was not found").await,
        };
        if let Err(e) = ctx.policy.check_storage(&job.account_id) {
            return fail(ctx, e).await;
        }
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some("Read storage transfer status".into()),
            None,
        )
        .await?;
        Ok(transfer_json(&job))
    }
}

pub struct StorageQuotaGetTool;

#[async_trait]
impl BuiltinTool for StorageQuotaGetTool {
    fn name(&self) -> &'static str {
        "storage_quota_get"
    }
    fn schema(&self) -> Value {
        transfer_schema(
            self.name(),
            "Read the quota of a connected cloud storage account.",
            json!({"account_id":{"type":"string"}}),
            &["account_id"],
        )
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::Low
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let quota = match ctx.storage.refresh_quota(account_id.clone()).await {
            Ok(quota) => quota,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some("Read storage quota".into()),
            None,
        )
        .await?;
        Ok(json!({"account_id": account_id, "quota": quota}))
    }
}

pub struct StorageFilesMoveTool;

#[async_trait]
impl BuiltinTool for StorageFilesMoveTool {
    fn name(&self) -> &'static str {
        "storage_files_move"
    }
    fn schema(&self) -> Value {
        transfer_schema(
            self.name(),
            "Move remote files to another remote folder. Requires approval.",
            json!({"account_id":{"type":"string"},"file_ids":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":100},"target_folder_id":{"type":["string","null"]}}),
            &["account_id", "file_ids"],
        )
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let file_ids = match parse_file_ids(ctx.args) {
            Ok(values) => values,
            Err(error) => return fail(ctx, error).await,
        };
        let target_folder_id = match optional_string(ctx.args, "target_folder_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        let count = match ctx
            .storage
            .move_files(account_id.clone(), file_ids, target_folder_id)
            .await
        {
            Ok(count) => count,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Moved {count} remote items")),
            None,
        )
        .await?;
        Ok(json!({"account_id": account_id, "moved": count}))
    }
}

pub struct StorageFilesTrashTool;

#[async_trait]
impl BuiltinTool for StorageFilesTrashTool {
    fn name(&self) -> &'static str {
        "storage_files_trash"
    }
    fn schema(&self) -> Value {
        transfer_schema(self.name(), "Move remote files to the provider trash. Permanent deletion is not supported by this tool.", json!({"account_id":{"type":"string"},"file_ids":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":100}}), &["account_id","file_ids"])
    }
    fn risk(&self, _: &Value) -> Risk {
        Risk::High
    }
    async fn execute(&self, ctx: &ToolCtx<'_>) -> AppResult<Value> {
        let account_id = match required_string(ctx.args, "account_id") {
            Ok(v) => v,
            Err(e) => return fail(ctx, e).await,
        };
        if let Err(e) = check_account(ctx, &account_id) {
            return fail(ctx, e).await;
        }
        let file_ids = match parse_file_ids(ctx.args) {
            Ok(values) => values,
            Err(error) => return fail(ctx, error).await,
        };
        let count = match ctx.storage.trash_files(account_id.clone(), file_ids).await {
            Ok(count) => count,
            Err(error) => return fail(ctx, error).await,
        };
        ctx.update_complete(
            "done",
            Some("success"),
            Some(0),
            Some(format!("Moved {count} remote items to trash")),
            None,
        )
        .await?;
        Ok(json!({"account_id": account_id, "trashed": count}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_argument_helpers_reject_malformed_values() {
        assert_eq!(
            optional_u64(&json!({"size": 12}), "size").unwrap(),
            Some(12)
        );
        assert!(optional_u64(&json!({"size": -1}), "size").is_err());
        assert!(optional_u64(&json!({"size": "12"}), "size").is_err());
        assert!(parse_file_ids(&json!({"file_ids": ["ok", 3]})).is_err());
        assert!(parse_file_ids(&json!({"file_ids": []})).is_err());
    }

    #[test]
    fn storage_argument_helpers_trim_file_ids() {
        assert_eq!(
            parse_file_ids(&json!({"file_ids": [" first ", "second"]})).unwrap(),
            vec!["first", "second"]
        );
    }

    #[test]
    fn transfer_results_do_not_expose_local_destination_paths() {
        let job = StorageTransferJobRow {
            id: "transfer-1".into(),
            account_id: "account-1".into(),
            operation: "file_download".into(),
            remote_item_id: Some("file-1".into()),
            display_name: "report.pdf".into(),
            destination_kind: Some("local_file".into()),
            destination_id: Some("/private/workspace/report.pdf".into()),
            status: "completed".into(),
            bytes_transferred: 10,
            bytes_total: Some(10),
            error_category: None,
            error_message: None,
            created_at: "2026-08-26T00:00:00Z".into(),
            updated_at: "2026-08-26T00:00:00Z".into(),
            completed_at: Some("2026-08-26T00:00:01Z".into()),
        };
        assert_eq!(transfer_json(&job)["destination_id"], Value::Null);
    }
}
