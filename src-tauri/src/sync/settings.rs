#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingClass {
    Syncable,
    DeviceLocal,
    Secret,
    Unknown,
}

pub const SYNC_GATEWAY_URL_SETTING: &str = "sync:gateway_url";

pub fn classify(key: &str) -> SettingClass {
    if key.starts_with("provider:") && key.ends_with(":api_key") {
        return SettingClass::Secret;
    }
    if key == crate::user_profile::USER_PROFILE_SETTING {
        return SettingClass::Syncable;
    }
    if key.starts_with("ui:")
        || key == "models:role_assignments"
        || key == "mcp:servers:v1"
        || key == "web:search_providers:v1"
        || key == SYNC_GATEWAY_URL_SETTING
        || key == crate::storage::artifact_cache::ARTIFACT_CACHE_QUOTA_SETTING
    {
        return SettingClass::DeviceLocal;
    }
    if key == crate::web::BRAVE_SEARCH_API_KEY_SECRET_ID
        || key == crate::web::TAVILY_SEARCH_API_KEY_SECRET_ID
        || key == "voice:dashscope:api_key"
    {
        return SettingClass::Secret;
    }
    if key.starts_with("voice:dashscope:") {
        return SettingClass::DeviceLocal;
    }
    SettingClass::Unknown
}

pub fn renderer_access_allowed(key: &str) -> bool {
    matches!(classify(key), SettingClass::DeviceLocal) && key.starts_with("ui:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_classified_without_secret_prefix_fallbacks() {
        assert_eq!(classify("ui:last_agent_id"), SettingClass::DeviceLocal);
        assert_eq!(
            classify(crate::user_profile::USER_PROFILE_SETTING),
            SettingClass::Syncable
        );
        assert_eq!(
            classify("models:role_assignments"),
            SettingClass::DeviceLocal
        );
        assert_eq!(classify("agent:agnes:memory_md"), SettingClass::Unknown);
        assert_eq!(classify("provider:openai:api_key"), SettingClass::Secret);
        assert_eq!(classify("mcp:servers:v1"), SettingClass::DeviceLocal);
        assert_eq!(
            classify(SYNC_GATEWAY_URL_SETTING),
            SettingClass::DeviceLocal
        );
        assert_eq!(
            classify("web:search_providers:v1"),
            SettingClass::DeviceLocal
        );
        assert_eq!(
            classify(crate::storage::artifact_cache::ARTIFACT_CACHE_QUOTA_SETTING),
            SettingClass::DeviceLocal
        );
        assert_eq!(classify("web:search:brave:api_key"), SettingClass::Secret);
        assert_eq!(classify("web:search:tavily:api_key"), SettingClass::Secret);
        assert_eq!(classify("unclassified:value"), SettingClass::Unknown);
        assert!(renderer_access_allowed("ui:last_session_id"));
        assert!(!renderer_access_allowed("provider:openai:api_key"));
        assert!(!renderer_access_allowed("agent:agnes:memory_md"));
    }
}
