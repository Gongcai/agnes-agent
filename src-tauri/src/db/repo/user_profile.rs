use crate::error::{AppError, AppResult};
use crate::sync::payload::SyncEntityType;
use crate::user_profile::{UserProfile, USER_PROFILE_SETTING};
use rusqlite::{Connection, Transaction, TransactionBehavior};

pub const SYNC_ENTITY_ID: &str = "user-profile-v1";
pub const VERSION_SETTING: &str = "user:profile:sync_version:v1";

pub fn save(conn: &mut Connection, profile: &UserProfile) -> AppResult<()> {
    profile.validate()?;
    let raw = serde_json::to_string(profile)
        .map_err(|error| AppError::Other(format!("用户资料无法序列化：{error}")))?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let next_version = next_local_version(&tx)?;
    crate::db::repo::settings::set(&tx, USER_PROFILE_SETTING, &raw)?;
    crate::db::repo::settings::set(&tx, VERSION_SETTING, &next_version.to_string())?;
    enqueue_current(&tx, profile, next_version)?;
    tx.commit()?;
    Ok(())
}

pub fn enqueue_existing(conn: &mut Connection) -> AppResult<()> {
    let Some(raw) = crate::db::repo::settings::get(conn, USER_PROFILE_SETTING)? else {
        return Ok(());
    };
    let profile: UserProfile = serde_json::from_str(&raw)
        .map_err(|error| AppError::Other(format!("用户资料无法解析：{error}")))?;
    profile.validate()?;
    let already_tracked: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_outbox WHERE entity_type = ?1 AND entity_id = ?2)
         OR EXISTS(SELECT 1 FROM sync_entity_state WHERE entity_type = ?1 AND entity_id = ?2)",
        (SyncEntityType::UserProfile.as_str(), SYNC_ENTITY_ID),
        |row| row.get(0),
    )?;
    if already_tracked != 0 {
        return Ok(());
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version = current_version(&tx)?.unwrap_or(1);
    crate::db::repo::settings::set(&tx, VERSION_SETTING, &version.to_string())?;
    enqueue_current(&tx, &profile, version)?;
    tx.commit()?;
    Ok(())
}

pub fn apply_remote(
    tx: &Transaction<'_>,
    profile: &UserProfile,
    remote_version: i64,
) -> AppResult<()> {
    profile.validate()?;
    let avatar = crate::db::repo::settings::get(tx, USER_PROFILE_SETTING)?
        .and_then(|raw| serde_json::from_str::<UserProfile>(&raw).ok())
        .map(|local| local.avatar)
        .unwrap_or_default();
    let mut applied = profile.clone();
    applied.avatar = avatar;
    let raw = serde_json::to_string(&applied)
        .map_err(|error| AppError::Other(format!("用户资料无法序列化：{error}")))?;
    let local_version = current_version(tx)?.unwrap_or(0);
    crate::db::repo::settings::set(tx, USER_PROFILE_SETTING, &raw)?;
    crate::db::repo::settings::set(
        tx,
        VERSION_SETTING,
        &local_version.max(remote_version).to_string(),
    )?;
    Ok(())
}

fn enqueue_current(conn: &Connection, profile: &UserProfile, version: i64) -> AppResult<()> {
    let device_id = crate::db::repo::sync::device_id(conn)?;
    let source = serde_json::json!({
        "id": SYNC_ENTITY_ID,
        "avatar": profile.avatar,
        "name": profile.name,
        "gender": profile.gender,
        "occupation": profile.occupation,
        "custom_occupation": profile.custom_occupation,
        "base_style": profile.base_style,
        "warmth": profile.warmth,
        "enthusiasm": profile.enthusiasm,
        "headings_lists": profile.headings_lists,
        "emoji": profile.emoji,
        "verbosity": profile.verbosity,
        "updated_at": version.to_string(),
        "version": version,
        "origin_device_id": device_id,
    });
    crate::db::repo::sync::enqueue_projection(
        conn,
        SyncEntityType::UserProfile,
        SYNC_ENTITY_ID,
        version,
        false,
        &source,
    )?;
    Ok(())
}

fn current_version(conn: &Connection) -> AppResult<Option<i64>> {
    Ok(crate::db::repo::settings::get(conn, VERSION_SETTING)?
        .and_then(|value| value.parse::<i64>().ok()))
}

fn next_local_version(conn: &Connection) -> AppResult<i64> {
    Ok(current_version(conn)?.unwrap_or(0).saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::schema::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO sync_runtime_state (singleton, device_id) VALUES (1, ?1)",
            ["00000000-0000-4000-8000-000000000001"],
        )
        .unwrap();
        conn
    }

    #[test]
    fn saving_profile_writes_an_allowlisted_sync_outbox_entry() {
        let mut conn = setup();
        let profile = UserProfile {
            avatar: "data:image/png;base64,local-avatar".into(),
            name: "小安".into(),
            occupation: "software_it".into(),
            base_style: "professional".into(),
            verbosity: "concise".into(),
            ..UserProfile::default()
        };

        save(&mut conn, &profile).unwrap();

        let stored: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                [USER_PROFILE_SETTING],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<UserProfile>(&stored).unwrap(),
            profile
        );
        let (entity_type, entity_id, version, payload): (String, String, i64, String) = conn
            .query_row(
                "SELECT entity_type, entity_id, local_version, payload FROM sync_outbox",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(entity_type, "user_profile");
        assert_eq!(entity_id, SYNC_ENTITY_ID);
        assert_eq!(version, 1);
        let payload: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(payload["name"], "小安");
        assert!(payload.get("avatar").is_none());
    }

    #[test]
    fn applying_remote_profile_preserves_local_avatar_and_does_not_enqueue_echo() {
        let mut conn = setup();
        let local = UserProfile {
            avatar: "data:image/png;base64,local-avatar".into(),
            ..UserProfile::default()
        };
        crate::db::repo::settings::set(
            &conn,
            USER_PROFILE_SETTING,
            &serde_json::to_string(&local).unwrap(),
        )
        .unwrap();
        let remote = UserProfile {
            name: "远端用户".into(),
            occupation: "student".into(),
            ..UserProfile::default()
        };
        let tx = conn.transaction().unwrap();
        apply_remote(&tx, &remote, 3).unwrap();
        tx.commit().unwrap();

        let stored = crate::db::repo::settings::get(&conn, USER_PROFILE_SETTING)
            .unwrap()
            .unwrap();
        let stored: UserProfile = serde_json::from_str(&stored).unwrap();
        assert_eq!(stored.name, "远端用户");
        assert_eq!(stored.occupation, "student");
        assert_eq!(stored.avatar, local.avatar);
        assert_eq!(
            crate::db::repo::settings::get(&conn, VERSION_SETTING).unwrap(),
            Some("3".into())
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
