use serde::{Deserialize, Serialize};

use crate::db::DbActorHandle;
use crate::error::{AppError, AppResult};

pub const USER_PROFILE_SETTING: &str = "user:profile:v1";

const MAX_AVATAR_LENGTH: usize = 5 * 1024 * 1024;
const MAX_SHORT_FIELD_LENGTH: usize = 120;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct UserProfile {
    pub avatar: String,
    pub name: String,
    pub gender: String,
    pub occupation: String,
    pub custom_occupation: String,
    pub base_style: String,
    pub warmth: String,
    pub enthusiasm: String,
    pub headings_lists: String,
    pub emoji: String,
    pub verbosity: String,
}

impl UserProfile {
    pub fn validate(&self) -> AppResult<()> {
        const ALLOWED_AVATAR_PREFIXES: [&str; 4] = [
            "data:image/png;base64,",
            "data:image/jpeg;base64,",
            "data:image/webp;base64,",
            "data:image/gif;base64,",
        ];
        if !self.avatar.is_empty()
            && (!ALLOWED_AVATAR_PREFIXES
                .iter()
                .any(|prefix| self.avatar.starts_with(prefix))
                || self.avatar.len() > MAX_AVATAR_LENGTH)
        {
            return Err(AppError::Other(
                "头像必须是 PNG、JPEG、WebP 或 GIF 格式，且不能超过 3 MB".into(),
            ));
        }
        for (label, value) in [
            ("姓名", &self.name),
            ("性别", &self.gender),
            ("职业", &self.occupation),
            ("自定义职业", &self.custom_occupation),
            ("基础风格", &self.base_style),
            ("温和体贴", &self.warmth),
            ("热情程度", &self.enthusiasm),
            ("标题和列表", &self.headings_lists),
            ("表情符号", &self.emoji),
            ("回答详略", &self.verbosity),
        ] {
            if value.chars().count() > MAX_SHORT_FIELD_LENGTH {
                return Err(AppError::Other(format!(
                    "{label}不能超过 {MAX_SHORT_FIELD_LENGTH} 个字符"
                )));
            }
        }
        validate_option(
            "性别",
            &self.gender,
            &[
                "",
                "male",
                "female",
                "nonbinary",
                "other",
                "prefer_not_to_say",
            ],
        )?;
        validate_option(
            "职业",
            &self.occupation,
            &[
                "",
                "software_it",
                "product_design",
                "education_research",
                "healthcare",
                "finance_law",
                "marketing_media",
                "management_entrepreneurship",
                "student",
                "freelancer",
                "other",
            ],
        )?;
        validate_option(
            "基础风格",
            &self.base_style,
            &[
                "",
                "default",
                "professional",
                "friendly",
                "candid",
                "quirky",
                "efficient",
                "nerdy",
                "cynical",
            ],
        )?;
        for (label, value) in [
            ("温和体贴", &self.warmth),
            ("热情洋溢", &self.enthusiasm),
            ("标题和列表", &self.headings_lists),
            ("表情符号", &self.emoji),
        ] {
            validate_option(label, value, &["", "default", "less", "more"])?;
        }
        validate_option(
            "回答详略",
            &self.verbosity,
            &["", "default", "concise", "detailed"],
        )?;
        Ok(())
    }

    pub fn to_markdown(&self) -> String {
        let mut lines = Vec::new();
        push_profile_line(&mut lines, "姓名", &inline_text(&self.name));
        push_profile_line(
            &mut lines,
            "性别",
            label_for(
                self.gender.trim(),
                &[
                    ("male", "男"),
                    ("female", "女"),
                    ("nonbinary", "非二元"),
                    ("other", "其他"),
                    ("prefer_not_to_say", "不愿透露"),
                ],
            ),
        );

        let occupation = if self.occupation.trim() == "other" {
            inline_text(&self.custom_occupation)
        } else {
            std::borrow::Cow::Borrowed(label_for(
                self.occupation.trim(),
                &[
                    ("software_it", "软件开发 / IT"),
                    ("product_design", "产品 / 设计"),
                    ("education_research", "教育 / 科研"),
                    ("healthcare", "医疗健康"),
                    ("finance_law", "金融 / 法律"),
                    ("marketing_media", "市场 / 媒体"),
                    ("management_entrepreneurship", "管理 / 创业"),
                    ("student", "学生"),
                    ("freelancer", "自由职业"),
                ],
            ))
        };
        push_profile_line(&mut lines, "职业", occupation.as_ref());

        push_preference_line(
            &mut lines,
            "基础风格和语调",
            self.base_style.trim(),
            &[
                ("professional", "专业可靠"),
                ("friendly", "亲和友善"),
                ("candid", "直言不讳"),
                ("quirky", "天马行空"),
                ("efficient", "高效务实"),
                ("nerdy", "探索求知"),
                ("cynical", "犀利幽默"),
            ],
        );
        push_preference_line(
            &mut lines,
            "温和体贴",
            self.warmth.trim(),
            &[("less", "较少"), ("more", "较多")],
        );
        push_preference_line(
            &mut lines,
            "热情洋溢",
            self.enthusiasm.trim(),
            &[("less", "较少"), ("more", "较多")],
        );
        push_preference_line(
            &mut lines,
            "标题和列表",
            self.headings_lists.trim(),
            &[("less", "较少使用"), ("more", "较多使用")],
        );
        push_preference_line(
            &mut lines,
            "表情符号",
            self.emoji.trim(),
            &[("less", "较少使用"), ("more", "较多使用")],
        );
        push_preference_line(
            &mut lines,
            "回答详略",
            self.verbosity.trim(),
            &[("concise", "简洁"), ("detailed", "详细")],
        );

        if lines.is_empty() {
            String::new()
        } else {
            format!("## 全局用户资料\n\n{}", lines.join("\n"))
        }
    }
}

fn validate_option(label: &str, value: &str, allowed: &[&str]) -> AppResult<()> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(AppError::Other(format!("{label}选项无效")))
    }
}

fn inline_text(value: &str) -> std::borrow::Cow<'_, str> {
    if value.contains(['\r', '\n']) {
        std::borrow::Cow::Owned(value.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        std::borrow::Cow::Borrowed(value.trim())
    }
}

fn label_for<'a>(value: &'a str, labels: &[(&str, &'static str)]) -> &'a str {
    labels
        .iter()
        .find_map(|(key, label)| (*key == value).then_some(*label))
        .unwrap_or(value)
}

fn push_profile_line(lines: &mut Vec<String>, label: &str, value: &str) {
    if !value.is_empty() && value != "default" {
        lines.push(format!("- {label}：{value}"));
    }
}

fn push_preference_line(
    lines: &mut Vec<String>,
    label: &str,
    value: &str,
    labels: &[(&str, &'static str)],
) {
    if !value.is_empty() && value != "default" {
        push_profile_line(lines, label, label_for(value, labels));
    }
}

pub async fn load_user_profile(db: &DbActorHandle) -> AppResult<UserProfile> {
    let Some(raw) = db.get_setting(USER_PROFILE_SETTING.to_string()).await? else {
        return Ok(UserProfile::default());
    };
    serde_json::from_str(&raw)
        .map_err(|error| AppError::Other(format!("用户资料无法解析：{error}")))
}

pub async fn save_user_profile(db: &DbActorHandle, profile: &UserProfile) -> AppResult<()> {
    profile.validate()?;
    let raw = serde_json::to_string(profile)
        .map_err(|error| AppError::Other(format!("用户资料无法序列化：{error}")))?;
    db.set_setting(USER_PROFILE_SETTING.to_string(), raw).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn populated_profile() -> UserProfile {
        UserProfile {
            avatar: "data:image/png;base64,avatar-data".into(),
            name: "小安".into(),
            occupation: "software_it".into(),
            base_style: "professional".into(),
            verbosity: "concise".into(),
            ..UserProfile::default()
        }
    }

    #[test]
    fn profile_markdown_omits_avatar_and_defaults() {
        let markdown = populated_profile().to_markdown();
        assert!(markdown.contains("- 姓名：小安"));
        assert!(markdown.contains("- 职业：软件开发 / IT"));
        assert!(markdown.contains("- 回答详略：简洁"));
        assert!(!markdown.contains("avatar-data"));
    }

    #[test]
    fn profile_markdown_keeps_user_text_on_one_line() {
        let markdown = UserProfile {
            name: "小安\n## 伪造标题".into(),
            occupation: "other".into(),
            custom_occupation: "工程师\n- 伪造列表".into(),
            ..UserProfile::default()
        }
        .to_markdown();
        assert!(markdown.contains("- 姓名：小安 ## 伪造标题"));
        assert!(markdown.contains("- 职业：工程师 - 伪造列表"));
        assert!(!markdown.contains("\n## 伪造标题"));
    }

    #[test]
    fn empty_profile_does_not_create_a_section() {
        assert_eq!(UserProfile::default().to_markdown(), "");
    }
}
