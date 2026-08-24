use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, RANGE};
use reqwest::{Method, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use zeroize::Zeroizing;

use super::domain::{
    BeginFileUploadRequest, ContentHashAlgorithm, CreateShareRequest, CreateShareResult,
    DownloadFileRequest, FileUploadSession, ImportSharedFilesRequest, ImportSharedFilesResult,
    ListFilesRequest, ProviderAuthKind, ProviderAuthorizationRequest, ProviderByteStream,
    ProviderDescriptor, ProviderError, ProviderErrorCategory, ProviderQuota, ProviderResult,
    ProviderStability, RemoteFileItem, RemoteFileKind, RemoteFilePage, SearchFilesRequest,
    SharedFileLink, StorageCapabilities, StorageProviderAccount, UploadFileChunkRequest,
    UploadedFileChunk,
};
use super::ports::{
    FileManagementProvider, FileShareProvider, FileSourceProvider, FileUploadProvider,
    ProviderAuthorizationChallenge, ProviderAuthorizationResult, ProviderAuthorizationStep,
    ProviderCredentialAccess, ProviderFactory, ProviderSession, QuotaProvider,
    SharedFileImportProvider,
};

const PROVIDER_ID: &str = "quark_drive";
const OPEN_API: &str = "https://open-api-drive.quark.cn";
const CLIENT_ID: &str = "third_party_agent";
const SIGN_KEY: &str = "cf134812e2de4032bd1cb7c3727e84b3";
const DEFAULT_DEVICE_ID: &str = "agnes_agent";
const DEFAULT_PLATFORM: &str = "linux";
const DEFAULT_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuarkCredential {
    access_token: String,
    refresh_token: Option<String>,
    device_id: String,
    user_id: String,
    #[serde(default = "default_token_type")]
    token_type: String,
    expires_at: u64,
}

fn default_token_type() -> String {
    "Bearer".into()
}

#[derive(Clone)]
struct AuthChallenge {
    page_code: String,
    client_device_id: String,
    expires_at: u64,
}

pub struct QuarkDriveFactory {
    client: reqwest::Client,
    sessions: StdMutex<HashMap<String, Weak<QuarkDriveSession>>>,
    challenges: StdMutex<HashMap<String, AuthChallenge>>,
}

impl QuarkDriveFactory {
    pub fn new() -> ProviderResult<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(20))
                .timeout(Duration::from_secs(120))
                .user_agent("Agnes-Agent/0.1 QuarkDriveProvider")
                .build()
                .map_err(network_error)?,
            sessions: StdMutex::new(HashMap::new()),
            challenges: StdMutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl ProviderFactory for QuarkDriveFactory {
    fn descriptor(&self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: PROVIDER_ID.into(),
            display_name: "夸克网盘".into(),
            auth_kind: ProviderAuthKind::OAuth2Pkce,
            stability: ProviderStability::Official,
            implementation_version: "quark-open-api-v1".into(),
            capabilities: StorageCapabilities {
                browse_files: false,
                search_files: true,
                read_files: true,
                write_files: true,
                delete_files: false,
                move_files: true,
                share_files: true,
                import_shared_files: true,
                range_download: true,
                resumable_upload: true,
                quota: true,
                user_authorization: true,
                recommended_chunk_bytes: Some(DEFAULT_CHUNK_BYTES),
                required_upload_hashes: vec![ContentHashAlgorithm::Md5, ContentHashAlgorithm::Sha1],
                ..StorageCapabilities::default()
            },
        }
    }

    async fn authorize(
        &self,
        request: ProviderAuthorizationRequest,
    ) -> ProviderResult<ProviderAuthorizationResult> {
        let code = request
            .input
            .get("agent_auth_code")
            .or_else(|| request.input.get("token"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| invalid_request("请提供夸克官方授权码"))?;
        let credential = exchange_agent_auth_code(&self.client, code).await?;
        authorized_result(&self.client, credential).await
    }

    async fn begin_authorization(
        &self,
        _request: ProviderAuthorizationRequest,
    ) -> ProviderResult<ProviderAuthorizationChallenge> {
        let client_device_id = format!("agnes-{}", uuid::Uuid::new_v4());
        let response = signed_request(
            &self.client,
            Method::POST,
            "/agent/v1/get_authorize_page_url",
            vec![("req_id".into(), request_id())],
            Some(json!({
                "client_device_id": client_device_id,
                "device_name": "Agnes Agent",
                "agent_id": "agnes-agent",
                "client_id": CLIENT_ID,
                "work_dir": "agnes-agent",
                "is_cloud_agent": false,
                "is_unsure_agent": false
            })),
        )
        .await?;
        let data = response_data(&response)?;
        let authorize_url = string_value(data, &["authorize_page_url", "url"])
            .ok_or_else(|| invalid_response("夸克官方授权接口未返回授权地址"))?;
        let page_code = string_value(data, &["page_code"])
            .ok_or_else(|| invalid_response("夸克官方授权接口未返回 page_code"))?;
        let challenge_id = uuid::Uuid::new_v4().to_string();
        self.challenges
            .lock()
            .map_err(|_| invalid_response("夸克授权会话不可用"))?
            .insert(
                challenge_id.clone(),
                AuthChallenge {
                    page_code: page_code.clone(),
                    client_device_id,
                    expires_at: now_epoch() + 300,
                },
            );
        Ok(ProviderAuthorizationChallenge {
            challenge_id,
            provider_id: PROVIDER_ID.into(),
            kind: "oauth_browser".into(),
            payload: json!({"authorize_url": authorize_url, "page_code": page_code}),
            expires_at: Some((now_epoch() + 300).to_string()),
        })
    }

    async fn poll_authorization(
        &self,
        challenge_id: &str,
    ) -> ProviderResult<ProviderAuthorizationStep> {
        let challenge = self
            .challenges
            .lock()
            .map_err(|_| invalid_response("夸克授权会话不可用"))?
            .get(challenge_id)
            .cloned()
            .ok_or_else(|| invalid_request("夸克授权会话不存在或已过期"))?;
        if now_epoch() >= challenge.expires_at {
            self.challenges
                .lock()
                .map_err(|_| invalid_response("夸克授权会话不可用"))?
                .remove(challenge_id);
            return Err(ProviderError::new(
                ProviderErrorCategory::Authentication,
                "夸克授权页面已过期，请重新授权",
            ));
        }
        let response = signed_request(
            &self.client,
            Method::GET,
            "/agent/v1/oauth/get_aac_by_pagecode",
            vec![
                ("req_id".into(), request_id()),
                ("page_code".into(), challenge.page_code),
            ],
            None,
        )
        .await?;
        let data = response_data(&response)?;
        if string_value(data, &["status"]).as_deref() != Some("success") {
            return Ok(ProviderAuthorizationStep::Pending);
        }
        let code = string_value(data, &["agent_auth_code"])
            .ok_or_else(|| invalid_response("夸克授权完成但未返回授权码"))?;
        let credential = exchange_agent_auth_code(&self.client, &code).await?;
        self.challenges
            .lock()
            .map_err(|_| invalid_response("夸克授权会话不可用"))?
            .remove(challenge_id);
        Ok(ProviderAuthorizationStep::Authorized(
            authorized_result(&self.client, credential).await?,
        ))
    }

    async fn connect(
        &self,
        account: &StorageProviderAccount,
        credentials: Arc<dyn ProviderCredentialAccess>,
    ) -> ProviderResult<Arc<dyn ProviderSession>> {
        if account.provider_id != PROVIDER_ID {
            return Err(invalid_request("夸克网盘收到其他 Provider 的账户"));
        }
        let stored = credentials.load().await?.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCategory::Authentication,
                "夸克官方授权不存在，请重新连接",
            )
        })?;
        parse_credential(stored.as_str())?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| invalid_response("夸克会话注册表不可用"))?;
        if let Some(session) = sessions.get(&account.id).and_then(Weak::upgrade) {
            return Ok(session);
        }
        let session = Arc::new(QuarkDriveSession {
            client: self.client.clone(),
            credentials,
            refresh_lock: Mutex::new(()),
            uploads: Mutex::new(HashMap::new()),
        });
        sessions.insert(account.id.clone(), Arc::downgrade(&session));
        Ok(session)
    }
}

struct QuarkDriveSession {
    client: reqwest::Client,
    credentials: Arc<dyn ProviderCredentialAccess>,
    refresh_lock: Mutex<()>,
    uploads: Mutex<HashMap<String, QuarkUploadState>>,
}

struct QuarkUploadState {
    task_id: String,
    part_size: u64,
    upload_urls: HashMap<u32, UploadUrl>,
    common_headers: HeaderMap,
    next_offset: u64,
    total_size: u64,
    part_etags: HashMap<u32, String>,
    instant_file: Option<RemoteFileItem>,
}

#[derive(Clone)]
struct UploadUrl {
    url: String,
    authorization: Option<String>,
}

#[async_trait]
impl FileSourceProvider for QuarkDriveSession {
    async fn list_files(&self, _request: ListFilesRequest) -> ProviderResult<RemoteFilePage> {
        Err(ProviderError::unsupported(
            "official Quark Drive folder browsing",
        ))
    }

    async fn search_files(&self, request: SearchFilesRequest) -> ProviderResult<RemoteFilePage> {
        let request = request.normalized();
        request.validate()?;
        let response = self.api(Method::POST, "/open/v1/file/search", vec![], Some(json!({
            "search_type": "mix", "keyword": request.query, "size": request.page_size,
            "page": request.page_token.as_deref().and_then(|v| v.parse::<u64>().ok()).unwrap_or(1)
        }))).await?;
        let data = response_data(&response)?;
        let list = data
            .get("file_list")
            .and_then(Value::as_array)
            .ok_or_else(|| invalid_response("夸克搜索结果格式无效"))?;
        let items = list
            .iter()
            .map(|item| remote_file(item, None))
            .collect::<ProviderResult<Vec<_>>>()?;
        let page = request
            .page_token
            .as_deref()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(1);
        Ok(RemoteFilePage {
            items,
            next_page_token: (list.len() >= request.page_size).then(|| (page + 1).to_string()),
        })
    }

    async fn get_file(&self, file_id: &str) -> ProviderResult<RemoteFileItem> {
        validate_id(file_id)?;
        let response = self
            .api(
                Method::GET,
                "/open/v1/file/info",
                vec![("fid".into(), file_id.into())],
                None,
            )
            .await?;
        remote_file(response_data(&response)?, None)
    }

    async fn download_file(
        &self,
        request: DownloadFileRequest,
    ) -> ProviderResult<ProviderByteStream> {
        validate_range(request.range_start, request.range_end_inclusive)?;
        let file = self.get_file(&request.file_id).await?;
        if request
            .expected_revision
            .as_deref()
            .zip(file.revision.as_deref())
            .is_some_and(|(a, b)| a != b)
        {
            return Err(ProviderError::new(
                ProviderErrorCategory::Conflict,
                "夸克网盘文件在下载前发生了变化",
            ));
        }
        let response = self
            .api(
                Method::POST,
                "/open/v1/file/get_download_url",
                vec![],
                Some(json!({"fid": request.file_id})),
            )
            .await?;
        let url = string_value(response_data(&response)?, &["download_url"])
            .ok_or_else(|| invalid_response("夸克官方接口未返回下载地址"))?;
        let mut headers = self.download_headers().await?;
        if let Some(value) = request.range_start {
            headers.insert(
                RANGE,
                HeaderValue::from_str(&format!(
                    "bytes={}-{}",
                    value,
                    request
                        .range_end_inclusive
                        .map(|v| v.to_string())
                        .unwrap_or_default()
                ))
                .map_err(|_| invalid_request("下载范围无效"))?,
            );
        }
        let response = self
            .client
            .get(url)
            .headers(headers)
            .send()
            .await
            .map_err(network_error)?;
        if request.range_start.is_some() && response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(invalid_response("夸克下载地址未响应 Range 请求"));
        }
        if !response.status().is_success() {
            return Err(status_error(response.status(), "夸克下载请求失败"));
        }
        response_stream(response)
    }
}

#[async_trait]
impl QuotaProvider for QuarkDriveSession {
    async fn quota(&self) -> ProviderResult<ProviderQuota> {
        let response = self
            .api(Method::GET, "/open/v1/user/get_vip_info", vec![], None)
            .await?;
        let data = response_data(&response)?;
        Ok(ProviderQuota {
            used_bytes: number_value(data, &["used", "use_capacity"]),
            total_bytes: number_value(data, &["capacity", "total_capacity"]),
            trashed_bytes: None,
            checked_at: now_epoch().to_string(),
        })
    }
}

#[async_trait]
impl FileManagementProvider for QuarkDriveSession {
    async fn trash_files(&self, _file_ids: Vec<String>) -> ProviderResult<()> {
        Err(ProviderError::unsupported(
            "official Quark Drive permanent or trash deletion",
        ))
    }

    async fn move_files(
        &self,
        file_ids: Vec<String>,
        target_folder_id: Option<String>,
    ) -> ProviderResult<()> {
        if file_ids.is_empty() || file_ids.len() > 100 {
            return Err(invalid_request("夸克一次最多移动 100 个项目"));
        }
        let target = target_folder_id.unwrap_or_else(|| "0".into());
        let response = self
            .api(
                Method::POST,
                "/open/v1/file/move",
                vec![],
                Some(json!({"fid_list": file_ids, "to_pdir_fid": target, "action_type": 1})),
            )
            .await?;
        let _ = response_data(&response)?;
        Ok(())
    }
}

#[async_trait]
impl FileShareProvider for QuarkDriveSession {
    async fn create_share(&self, request: CreateShareRequest) -> ProviderResult<CreateShareResult> {
        if request.file_ids.is_empty() || request.file_ids.len() > 100 {
            return Err(invalid_request("夸克一次最多分享 100 个项目"));
        }
        if request.expires_at.is_some() {
            return Err(ProviderError::unsupported(
                "夸克分享有效期需要使用官方 expired_type，通用接口暂不支持日期映射",
            ));
        }
        let mut body =
            json!({"fid_list": request.file_ids, "title": request.title.unwrap_or_default()});
        if let Some(password) = request.password {
            body["passcode"] = json!(password);
        }
        let response = self
            .api(Method::POST, "/agent/v1/share/create", vec![], Some(body))
            .await?;
        let data = response_data(&response)?;
        let url = string_value(data, &["share_url"])
            .ok_or_else(|| invalid_response("夸克未返回分享链接"))?;
        Ok(CreateShareResult {
            links: vec![SharedFileLink {
                file_id: string_value(data, &["fid", "share_id"]).unwrap_or_default(),
                url,
                password: string_value(data, &["passcode"]),
                expires_at: None,
            }],
        })
    }
}

#[async_trait]
impl SharedFileImportProvider for QuarkDriveSession {
    async fn import_shared_files(
        &self,
        request: ImportSharedFilesRequest,
    ) -> ProviderResult<ImportSharedFilesResult> {
        let (pwd_id, embedded_password) = parse_quark_share_url(&request.share_url)?;
        let password = request.password.or(embedded_password);
        let selected = request.file_ids.unwrap_or_default();
        if request.save_all && !selected.is_empty() {
            return Err(invalid_request("夸克转存不能同时指定 save_all 和 file_ids"));
        }
        let detail = self.api(Method::POST, "/open/v1/share/detail", vec![], Some(json!({"pwd_id": pwd_id, "page": 1, "size": 200, "force": 0, "passcode": password}))).await?;
        let detail_data = response_data(&detail)?;
        let stoken = string_value(
            detail_data.get("token_info").unwrap_or(detail_data),
            &["stoken"],
        )
        .ok_or_else(|| invalid_response("夸克分享详情未返回 stoken"))?;
        let save_all = request.save_all || selected.is_empty();
        let list = detail_data
            .get("detail_info")
            .and_then(|value| value.get("list"))
            .and_then(Value::as_array)
            .map_or(&[][..], |value| value);
        let fid_tokens = if save_all {
            None
        } else {
            let token_by_fid = list
                .iter()
                .filter_map(|item| {
                    Some((
                        string_value(item, &["fid"])?,
                        string_value(item, &["share_fid_token"])?,
                    ))
                })
                .collect::<HashMap<_, _>>();
            let tokens = selected
                .iter()
                .map(|fid| token_by_fid.get(fid).cloned())
                .collect::<Option<Vec<_>>>();
            tokens
                .ok_or_else(|| invalid_request("指定的夸克分享文件不在当前分享详情中"))?
                .into()
        };
        let mut body = json!({"pwd_id": pwd_id, "stoken": stoken, "save_all": save_all});
        if !save_all {
            body["fid_list"] = json!(selected);
        }
        if let Some(folder_id) = request.target_folder_id {
            body["to_pdir_fid"] = json!(folder_id);
        }
        if let Some(tokens) = fid_tokens {
            body["fid_token_list"] = json!(tokens);
        }
        let response = self
            .api(Method::POST, "/open/v1/share/saveas", vec![], Some(body))
            .await?;
        let data = response_data(&response)?;
        let task_id = string_value(data, &["task_id"])
            .ok_or_else(|| invalid_response("夸克转存接口未返回 task_id"))?;
        let task = self.wait_for_task(&task_id).await?;
        let completed = number_value(&task, &["status"]).is_some_and(|status| status == 2);
        if !completed {
            return Err(ProviderError::new(
                ProviderErrorCategory::RemoteUnavailable,
                "夸克转存任务未成功完成",
            ));
        }
        Ok(ImportSharedFilesResult {
            task_id: Some(task_id),
            files: Vec::new(),
            completed,
        })
    }
}

impl ProviderSession for QuarkDriveSession {
    fn file_source(&self) -> Option<&dyn FileSourceProvider> {
        Some(self)
    }
    fn quota_source(&self) -> Option<&dyn QuotaProvider> {
        Some(self)
    }
    fn file_upload(&self) -> Option<&dyn FileUploadProvider> {
        Some(self)
    }
    fn file_management(&self) -> Option<&dyn FileManagementProvider> {
        Some(self)
    }
    fn file_sharing(&self) -> Option<&dyn FileShareProvider> {
        Some(self)
    }
    fn shared_file_import(&self) -> Option<&dyn SharedFileImportProvider> {
        Some(self)
    }
}

#[async_trait]
impl FileUploadProvider for QuarkDriveSession {
    async fn begin_file_upload(
        &self,
        request: BeginFileUploadRequest,
    ) -> ProviderResult<FileUploadSession> {
        let md5 = request
            .content_hashes
            .get(&ContentHashAlgorithm::Md5)
            .cloned()
            .unwrap_or_default();
        let sha1 = request
            .content_hashes
            .get(&ContentHashAlgorithm::Sha1)
            .cloned()
            .unwrap_or_default();
        if md5.len() != 32 || sha1.len() != 40 {
            return Err(invalid_request("夸克官方上传需要 MD5 和 SHA1"));
        }
        let response = self
            .api(
                Method::POST,
                "/open/v1/file/upload_pre",
                vec![],
                Some(json!({
                    "file_name": request.name, "size": request.size, "sha1": sha1, "md5": md5,
                    "pdir_fid": request.parent_id, "format_type": request.media_type,
                    "parallel_upload": true, "hash_update": false
                })),
            )
            .await?;
        let data = response_data(&response)?;
        if bool_value(data, &["finish"]).unwrap_or(false) {
            let fid = string_value(data, &["fid"])
                .ok_or_else(|| invalid_response("夸克秒传响应未返回 fid"))?;
            let file = self.get_file(&fid).await?;
            let session_id = format!("quark-upload-{}", uuid::Uuid::new_v4());
            self.uploads.lock().await.insert(
                session_id.clone(),
                QuarkUploadState {
                    task_id: String::new(),
                    part_size: request.chunk_size.max(1),
                    upload_urls: HashMap::new(),
                    common_headers: HeaderMap::new(),
                    next_offset: 0,
                    total_size: request.size,
                    part_etags: HashMap::new(),
                    instant_file: Some(file),
                },
            );
            return Ok(FileUploadSession {
                session_id,
                next_offset: 0,
                chunk_size: request.chunk_size.max(1),
                expires_at: None,
            });
        }
        let task_id = string_value(data, &["task_id"])
            .ok_or_else(|| invalid_response("夸克预上传未返回 task_id"))?;
        let part_size = number_value(data, &["part_size"])
            .unwrap_or(request.chunk_size)
            .max(1);
        let upload_urls = match data.get("upload_urls") {
            Some(_) => parse_upload_urls(data)?,
            None => HashMap::new(),
        };
        let session_id = format!("quark-upload-{}", uuid::Uuid::new_v4());
        let common_headers = parse_headers(data.get("common_headers"));
        self.uploads.lock().await.insert(
            session_id.clone(),
            QuarkUploadState {
                task_id,
                part_size,
                upload_urls,
                common_headers,
                next_offset: 0,
                total_size: request.size,
                part_etags: HashMap::new(),
                instant_file: None,
            },
        );
        Ok(FileUploadSession {
            session_id,
            next_offset: 0,
            chunk_size: part_size,
            expires_at: None,
        })
    }

    async fn upload_file_chunk(
        &self,
        request: UploadFileChunkRequest,
    ) -> ProviderResult<UploadedFileChunk> {
        let (part_size, instant_upload) = {
            let uploads = self.uploads.lock().await;
            let state = uploads
                .get(&request.session_id)
                .ok_or_else(|| invalid_request("夸克上传会话不存在或已过期"))?;
            (state.part_size, state.instant_file.is_some())
        };
        let part_number = (request.offset / part_size) as u32 + 1;
        let url = if instant_upload {
            None
        } else {
            Some(
                self.ensure_upload_url(
                    &request.session_id,
                    part_number,
                    request.bytes.len() as u64,
                )
                .await?,
            )
        };
        let mut uploads = self.uploads.lock().await;
        let state = uploads
            .get_mut(&request.session_id)
            .ok_or_else(|| invalid_request("夸克上传会话不存在或已过期"))?;
        if request.offset != state.next_offset || request.total_size != state.total_size {
            return Err(invalid_request("夸克上传偏移无效"));
        }
        if let Some(file) = state.instant_file.clone() {
            state.next_offset = state.next_offset.saturating_add(request.bytes.len() as u64);
            if state.next_offset != state.total_size {
                return Ok(UploadedFileChunk {
                    next_offset: state.next_offset,
                    complete: false,
                    file: None,
                });
            }
            let total_size = state.total_size;
            uploads.remove(&request.session_id);
            return Ok(UploadedFileChunk {
                next_offset: total_size,
                complete: true,
                file: Some(file),
            });
        }
        let url = url.ok_or_else(|| invalid_response("夸克上传会话缺少分片地址"))?;
        let mut upload = self
            .client
            .put(url.url)
            .headers(state.common_headers.clone())
            .body(request.bytes.clone());
        if let Some(signature) = url.authorization {
            upload = upload.header("authorization", signature);
        }
        let response = upload.send().await.map_err(network_error)?;
        if !response.status().is_success() {
            return Err(status_error(response.status(), "夸克分片上传失败"));
        }
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .ok_or_else(|| invalid_response("夸克分片上传响应缺少 ETag"))?;
        state.part_etags.insert(part_number, etag);
        state.next_offset = state.next_offset.saturating_add(request.bytes.len() as u64);
        if state.next_offset < state.total_size {
            return Ok(UploadedFileChunk {
                next_offset: state.next_offset,
                complete: false,
                file: None,
            });
        }
        let parts = (1..=part_number).map(|part| {
            json!({"part_number": part, "etag": state.part_etags.get(&part).cloned().unwrap_or_default()})
        }).collect::<Vec<_>>();
        let task_id = state.task_id.clone();
        let total_size = state.total_size;
        let finish = self
            .api(
                Method::POST,
                "/open/v1/file/upload_finish",
                vec![],
                Some(json!({"task_id": task_id, "part_info_list": parts})),
            )
            .await?;
        let data = response_data(&finish)?;
        if !bool_value(data, &["finish"]).unwrap_or(false) {
            return Err(invalid_response("夸克上传完成接口未确认 finish"));
        }
        let fid = string_value(data, &["fid"])
            .ok_or_else(|| invalid_response("夸克上传完成接口未返回 fid"))?;
        let file = self.get_file(&fid).await?;
        uploads.remove(&request.session_id);
        Ok(UploadedFileChunk {
            next_offset: total_size,
            complete: true,
            file: Some(file),
        })
    }

    async fn abort_file_upload(&self, session_id: &str) -> ProviderResult<()> {
        self.uploads.lock().await.remove(session_id);
        Ok(())
    }
}

impl QuarkDriveSession {
    async fn upload_part_size(&self, session_id: &str) -> ProviderResult<u64> {
        self.uploads
            .lock()
            .await
            .get(session_id)
            .map(|state| state.part_size)
            .ok_or_else(|| invalid_request("夸克上传会话不存在或已过期"))
    }

    async fn ensure_upload_url(
        &self,
        session_id: &str,
        part_number: u32,
        requested_size: u64,
    ) -> ProviderResult<UploadUrl> {
        if let Some(url) = self
            .uploads
            .lock()
            .await
            .get(session_id)
            .and_then(|state| state.upload_urls.get(&part_number).cloned())
        {
            return Ok(url);
        }
        let task_id = {
            let uploads = self.uploads.lock().await;
            let state = uploads
                .get(session_id)
                .ok_or_else(|| invalid_request("夸克上传会话不存在或已过期"))?;
            state.task_id.clone()
        };
        let response = self.api(
            Method::POST,
            "/open/v1/file/get_upload_urls",
            vec![],
            Some(json!({"task_id": task_id, "part_info_list": [{"part_number": part_number, "part_size": requested_size}]})),
        ).await?;
        let data = response_data(&response)?;
        let urls = parse_upload_urls(data)?;
        let url = urls
            .get(&part_number)
            .cloned()
            .ok_or_else(|| invalid_response("夸克官方接口未返回当前分片地址"))?;
        let mut uploads = self.uploads.lock().await;
        let state = uploads
            .get_mut(session_id)
            .ok_or_else(|| invalid_request("夸克上传会话不存在或已过期"))?;
        state.upload_urls.extend(urls);
        let headers = parse_headers(data.get("common_headers"));
        if !headers.is_empty() {
            state.common_headers = headers;
        }
        Ok(url)
    }

    async fn api(
        &self,
        method: Method,
        path: &str,
        query: Vec<(String, String)>,
        body: Option<Value>,
    ) -> ProviderResult<Value> {
        let mut retried = false;
        loop {
            let credential = self.credential(false).await?;
            let mut query = query.clone();
            query.push(("req_id".into(), request_id()));
            query.push(("access_token".into(), credential.access_token.clone()));
            query.push(("device_id".into(), credential.device_id.clone()));
            query.push(("platform".into(), DEFAULT_PLATFORM.into()));
            let response =
                signed_request(&self.client, method.clone(), path, query, body.clone()).await;
            match response {
                Ok(value) => return Ok(value),
                Err(error)
                    if error.category == ProviderErrorCategory::Authentication && !retried =>
                {
                    self.credential(true).await?;
                    retried = true;
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn credential(&self, force_refresh: bool) -> ProviderResult<QuarkCredential> {
        let _guard = self.refresh_lock.lock().await;
        let stored = self.credentials.load().await?.ok_or_else(|| {
            ProviderError::new(ProviderErrorCategory::Authentication, "夸克官方授权不存在")
        })?;
        let mut credential = parse_credential(stored.as_str())?;
        if force_refresh || credential.expires_at <= now_epoch() + 60 {
            let refresh = credential.refresh_token.clone().ok_or_else(|| {
                ProviderError::new(
                    ProviderErrorCategory::Authentication,
                    "夸克授权已过期，请重新授权",
                )
            })?;
            let response = signed_request(
                &self.client,
                Method::POST,
                "/agent/v1/oauth/access_token/rotate",
                vec![("req_id".into(), request_id())],
                Some(json!({"refresh_token": refresh, "device_id": credential.device_id})),
            )
            .await?;
            let data = response_data(&response)?;
            credential.access_token = string_value(data, &["access_token"])
                .ok_or_else(|| invalid_response("夸克刷新接口未返回 access_token"))?;
            credential.refresh_token = string_value(data, &["refresh_token"]);
            credential.expires_at =
                now_epoch() + number_value(data, &["expires_in"]).unwrap_or(3600);
            self.credentials
                .store(Zeroizing::new(
                    serde_json::to_string(&credential)
                        .map_err(|_| invalid_response("夸克授权编码失败"))?,
                ))
                .await?;
        }
        Ok(credential)
    }

    async fn download_headers(&self) -> ProviderResult<HeaderMap> {
        let credential = self.credential(false).await?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            HeaderValue::from_str(&format!(
                "x_pan_client_id={CLIENT_ID};x_pan_access_token={}",
                credential.access_token
            ))
            .map_err(|_| invalid_request("夸克下载凭证无效"))?,
        );
        Ok(headers)
    }

    async fn wait_for_task(&self, task_id: &str) -> ProviderResult<Value> {
        for _ in 0..900 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let response = self
                .api(
                    Method::POST,
                    "/open/v1/task/query",
                    vec![],
                    Some(json!({"task_id": task_id})),
                )
                .await?;
            let data = response_data(&response)?;
            let status = number_value(data, &["status"]);
            if matches!(status, Some(2 | 3 | 4)) {
                return Ok(data.clone());
            }
        }
        Err(ProviderError::new(
            ProviderErrorCategory::RemoteUnavailable,
            "夸克转存任务轮询超时",
        ))
    }
}

async fn exchange_agent_auth_code(
    client: &reqwest::Client,
    code: &str,
) -> ProviderResult<QuarkCredential> {
    let response = signed_request(
        client,
        Method::GET,
        "/agent/v1/oauth/agent_auth_code",
        vec![
            ("req_id".into(), request_id()),
            ("agent_auth_code".into(), code.into()),
        ],
        None,
    )
    .await?;
    let data = response_data(&response)?;
    let access_token = string_value(data, &["access_token"]).ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCategory::Authentication,
            "夸克授权码未返回 access_token",
        )
    })?;
    let user_id = string_value(data, &["user_id"])
        .ok_or_else(|| invalid_response("夸克授权未返回 user_id"))?;
    Ok(QuarkCredential {
        access_token,
        refresh_token: string_value(data, &["refresh_token"]),
        device_id: string_value(data, &["device_id"]).unwrap_or_else(|| DEFAULT_DEVICE_ID.into()),
        user_id,
        token_type: default_token_type(),
        expires_at: now_epoch() + number_value(data, &["expires_in"]).unwrap_or(3600),
    })
}

async fn authorized_result(
    client: &reqwest::Client,
    credential: QuarkCredential,
) -> ProviderResult<ProviderAuthorizationResult> {
    let response = signed_request(
        client,
        Method::GET,
        "/open/v1/user/info",
        vec![
            ("req_id".into(), request_id()),
            ("access_token".into(), credential.access_token.clone()),
            ("device_id".into(), credential.device_id.clone()),
            ("platform".into(), DEFAULT_PLATFORM.into()),
        ],
        None,
    )
    .await?;
    let data = response_data(&response)?;
    let nickname = string_value(data, &["nickname"]).unwrap_or_else(|| credential.user_id.clone());
    Ok(ProviderAuthorizationResult {
        account: StorageProviderAccount {
            id: format!("quark-{}", short_hash(credential.user_id.as_bytes())),
            provider_id: PROVIDER_ID.into(),
            display_name: format!("夸克网盘 - {}", truncate(&nickname, 80)),
            account_subject: Some(credential.user_id.clone()),
            config: json!({"api_base": OPEN_API, "official_api": true, "credential_kind": "oauth_agent"}),
        },
        credential: Zeroizing::new(
            serde_json::to_string(&credential).map_err(|_| invalid_response("夸克授权编码失败"))?,
        ),
    })
}

async fn signed_request(
    client: &reqwest::Client,
    method: Method,
    path: &str,
    query: Vec<(String, String)>,
    body: Option<Value>,
) -> ProviderResult<Value> {
    let timestamp = now_millis().to_string();
    let signature = hex_encode(&Sha256::digest(
        format!(
            "{}&{}&{}&{}",
            method.as_str().to_uppercase(),
            path,
            timestamp,
            SIGN_KEY
        )
        .as_bytes(),
    ));
    let mut headers = HeaderMap::new();
    headers.insert("x-pan-client-id", HeaderValue::from_static(CLIENT_ID));
    headers.insert(
        "x-pan-tm",
        HeaderValue::from_str(&timestamp).map_err(|_| invalid_request("夸克请求时间戳无效"))?,
    );
    headers.insert(
        "x-pan-token",
        HeaderValue::from_str(&signature).map_err(|_| invalid_request("夸克请求签名无效"))?,
    );
    headers.insert("accept", HeaderValue::from_static("application/json"));
    let response = client
        .request(method, format!("{OPEN_API}{path}"))
        .headers(headers)
        .query(&query)
        .json(&body.unwrap_or(Value::Null))
        .send()
        .await
        .map_err(network_error)?;
    let status = response.status();
    let value: Value = response
        .json()
        .await
        .map_err(|_| invalid_response("夸克官方接口返回非 JSON"))?;
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ProviderError::new(
            ProviderErrorCategory::Authentication,
            "夸克官方授权已失效",
        ));
    }
    if !status.is_success() {
        return Err(status_error(status, "夸克官方接口请求失败"));
    }
    if value
        .get("status")
        .and_then(Value::as_i64)
        .is_some_and(|status| status != 0)
    {
        return Err(ProviderError::new(
            ProviderErrorCategory::RemoteUnavailable,
            string_value(&value, &["error_info", "agent_msg"])
                .unwrap_or_else(|| "夸克官方接口请求失败".into()),
        ));
    }
    Ok(value)
}

fn response_data(value: &Value) -> ProviderResult<&Value> {
    value
        .get("data")
        .ok_or_else(|| invalid_response("夸克官方响应缺少 data"))
}
fn parse_credential(value: &str) -> ProviderResult<QuarkCredential> {
    let credential: QuarkCredential = serde_json::from_str(value).map_err(|_| {
        ProviderError::new(
            ProviderErrorCategory::Authentication,
            "夸克官方授权凭证格式无效",
        )
    })?;
    if credential.access_token.is_empty() || credential.user_id.is_empty() {
        return Err(ProviderError::new(
            ProviderErrorCategory::Authentication,
            "夸克官方授权凭证不完整",
        ));
    }
    Ok(credential)
}
fn remote_file(value: &Value, parent_id: Option<&str>) -> ProviderResult<RemoteFileItem> {
    let id = string_value(value, &["fid", "file_id", "id"])
        .ok_or_else(|| invalid_response("夸克文件缺少 fid"))?;
    let name =
        string_value(value, &["file_name", "filename", "name"]).unwrap_or_else(|| id.clone());
    let folder = bool_value(value, &["dir", "is_dir", "is_folder"]).unwrap_or_else(|| {
        matches!(
            string_value(value, &["file_type"]).as_deref(),
            Some("0" | "folder")
        )
    });
    Ok(RemoteFileItem {
        id,
        parent_id: parent_id
            .map(str::to_owned)
            .or_else(|| string_value(value, &["pdir_fid", "parent_id"])),
        name,
        kind: if folder {
            RemoteFileKind::Folder
        } else {
            RemoteFileKind::File
        },
        media_type: string_value(value, &["mime_type", "format_type"]),
        size: number_value(value, &["size", "file_size"]),
        modified_at: string_value(value, &["updated_at", "modified_at"]),
        revision: string_value(value, &["sha1", "md5"]),
        downloadable: !folder,
    })
}
fn parse_upload_urls(value: &Value) -> ProviderResult<HashMap<u32, UploadUrl>> {
    let urls = value
        .get("upload_urls")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_response("夸克预上传未返回 upload_urls"))?;
    let mut result = HashMap::new();
    for item in urls {
        let part = number_value(item, &["part_number"])
            .ok_or_else(|| invalid_response("夸克分片编号无效"))? as u32;
        let url = string_value(item, &["upload_url", "url"])
            .ok_or_else(|| invalid_response("夸克分片地址无效"))?;
        let authorization = item
            .get("signature_info")
            .and_then(|value| string_value(value, &["signature"]));
        result.insert(part, UploadUrl { url, authorization });
    }
    if result.is_empty() {
        return Err(invalid_response("夸克预上传未返回有效分片地址"));
    }
    Ok(result)
}
fn parse_headers(value: Option<&Value>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(Value::Object(values)) = value {
        for (name, value) in values {
            if let Some(value) = value.as_str() {
                if let (Ok(name), Ok(value)) =
                    (name.parse::<HeaderName>(), HeaderValue::from_str(value))
                {
                    headers.insert(name, value);
                }
            }
        }
    }
    headers
}
fn parse_quark_share_url(value: &str) -> ProviderResult<(String, Option<String>)> {
    let url = Url::parse(value).map_err(|_| invalid_request("夸克分享链接无效"))?;
    if !matches!(
        url.host_str(),
        Some("pan.quark.cn") | Some("www.pan.quark.cn")
    ) {
        return Err(invalid_request("不是夸克网盘分享链接"));
    }
    let mut segments = url
        .path_segments()
        .ok_or_else(|| invalid_request("夸克分享链接缺少分享 ID"))?;
    if segments.next() != Some("s") {
        return Err(invalid_request("夸克分享链接缺少分享 ID"));
    }
    let id = segments
        .next()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| invalid_request("夸克分享链接缺少分享 ID"))?;
    let password = url
        .query_pairs()
        .find(|(k, _)| k == "pwd")
        .map(|(_, v)| v.into_owned());
    Ok((id, password))
}
fn string_value(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        })
    })
}
fn number_value(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
    })
}
fn bool_value(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_bool()
                .or_else(|| v.as_i64().map(|n| n != 0))
                .or_else(|| v.as_str().map(|s| s == "1" || s == "true"))
        })
    })
}
fn validate_id(value: &str) -> ProviderResult<()> {
    if value.is_empty() || value.len() > 1024 || value.chars().any(char::is_control) {
        Err(invalid_request("夸克文件 ID 无效"))
    } else {
        Ok(())
    }
}
fn validate_range(start: Option<u64>, end: Option<u64>) -> ProviderResult<()> {
    if end.is_some() && start.is_none() || start.zip(end).is_some_and(|(a, b)| b < a) {
        Err(invalid_request("下载范围无效"))
    } else {
        Ok(())
    }
}
fn response_stream(response: Response) -> ProviderResult<ProviderByteStream> {
    Ok(Box::pin(response.bytes_stream().map(|chunk| {
        chunk.map(|bytes| bytes.to_vec()).map_err(network_error)
    })))
}
fn status_error(status: StatusCode, message: &str) -> ProviderError {
    ProviderError::new(
        if status == StatusCode::TOO_MANY_REQUESTS {
            ProviderErrorCategory::RateLimit
        } else if status.is_server_error() {
            ProviderErrorCategory::RemoteUnavailable
        } else {
            ProviderErrorCategory::Network
        },
        format!("{message} ({})", status.as_u16()),
    )
}
fn network_error(error: reqwest::Error) -> ProviderError {
    ProviderError::new(
        ProviderErrorCategory::Network,
        format!("夸克网络请求失败: {error}"),
    )
}
fn invalid_request(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCategory::InvalidRequest, message)
}
fn invalid_response(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCategory::InvalidResponse, message)
}
fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}
fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn short_hash(value: &[u8]) -> String {
    hex_encode(&Sha256::digest(value)[..10])
}
fn truncate(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quark_share_urls_and_embedded_passwords() {
        assert_eq!(
            parse_quark_share_url("https://pan.quark.cn/s/abc123").unwrap(),
            ("abc123".into(), None)
        );
        assert_eq!(
            parse_quark_share_url("https://www.pan.quark.cn/s/abc123?pwd=a%2Bb").unwrap(),
            ("abc123".into(), Some("a+b".into()))
        );
        assert!(parse_quark_share_url("https://pan.quark.cn/abc123").is_err());
        assert!(parse_quark_share_url("https://example.com/s/abc123").is_err());
    }

    #[test]
    fn parses_upload_urls_and_common_headers() {
        let value = json!({
            "upload_urls": [{
                "part_number": 1,
                "upload_url": "https://upload.example/part-1",
                "signature_info": {"signature": "signed"}
            }],
            "common_headers": {"x-test": "value"}
        });
        let urls = parse_upload_urls(&value).unwrap();
        assert_eq!(
            urls.get(&1).and_then(|item| item.authorization.as_deref()),
            Some("signed")
        );
        assert_eq!(
            parse_headers(value.get("common_headers"))
                .get("x-test")
                .and_then(|value| value.to_str().ok()),
            Some("value")
        );
    }

    #[test]
    fn recognizes_quark_folder_file_type() {
        let folder = remote_file(
            &json!({"fid": "folder", "file_name": "docs", "file_type": 0}),
            None,
        )
        .unwrap();
        assert_eq!(folder.kind, RemoteFileKind::Folder);
        assert!(!folder.downloadable);
    }

    #[test]
    fn official_descriptor_exposes_share_and_import_without_folder_browse() {
        let factory = QuarkDriveFactory::new().unwrap();
        let descriptor = factory.descriptor();
        assert!(!descriptor.capabilities.browse_files);
        assert!(descriptor.capabilities.share_files);
        assert!(descriptor.capabilities.import_shared_files);
        assert_eq!(descriptor.stability, ProviderStability::Official);
    }
}
