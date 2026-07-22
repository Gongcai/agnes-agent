#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingClass {
    Syncable,
    DeviceLocal,
    Secret,
    Unknown,
}

pub fn classify(key: &str) -> SettingClass {
    if key.starts_with("provider:") && key.ends_with(":api_key") {
        return SettingClass::Secret;
    }
    if key.starts_with("ui:")
        || key == crate::user_profile::USER_PROFILE_SETTING
        || key.starts_with(crate::user_profile::AGENT_USER_PROFILE_MODE_PREFIX)
        || key == "models:role_assignments"
        || key == "mcp:servers:v1"
        || key == "web:search_providers:v1"
        || key == crate::storage::artifact_cache::ARTIFACT_CACHE_QUOTA_SETTING
    {
        return SettingClass::DeviceLocal;
    }
    if key == "web:search:brave:api_key" {
        return SettingClass::Secret;
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
            SettingClass::DeviceLocal
        );
        assert_eq!(
            classify("agent:user_profile_mode:agnes"),
            SettingClass::DeviceLocal
        );
        assert_eq!(
            classify("models:role_assignments"),
            SettingClass::DeviceLocal
        );
        assert_eq!(classify("agent:agnes:memory_md"), SettingClass::Unknown);
        assert_eq!(classify("provider:openai:api_key"), SettingClass::Secret);
        assert_eq!(classify("mcp:servers:v1"), SettingClass::DeviceLocal);
        assert_eq!(
            classify("web:search_providers:v1"),
            SettingClass::DeviceLocal
        );
        assert_eq!(
            classify(crate::storage::artifact_cache::ARTIFACT_CACHE_QUOTA_SETTING),
            SettingClass::DeviceLocal
        );
        assert_eq!(classify("web:search:brave:api_key"), SettingClass::Secret);
        assert_eq!(classify("unclassified:value"), SettingClass::Unknown);
        assert!(renderer_access_allowed("ui:last_session_id"));
        assert!(!renderer_access_allowed("provider:openai:api_key"));
        assert!(!renderer_access_allowed("agent:agnes:memory_md"));
    }
}
