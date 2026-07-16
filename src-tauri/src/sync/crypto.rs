use std::collections::BTreeMap;

use base64::{
    engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD},
    Engine as _,
};
use chacha20poly1305::{
    aead::{Aead, Generate, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{AppError, AppResult};

pub const PAYLOAD_ENCODING: &str = "xchacha20poly1305-v1";
pub const TOMBSTONE_ENCODING: &str = "tombstone-v1";
pub const EMPTY_PAYLOAD_HASH: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
pub const KEYSET_FORMAT_VERSION: u8 = 1;
const MASTER_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const MAX_KEY_VERSIONS: usize = 32;
const AAD_DOMAIN: &[u8] = b"agnes-sync-payload-aad-v1\0";
const RECOVERY_AAD: &[u8] = b"agnes-sync-recovery-bundle-v1\0";
const RECOVERY_INFO: &[u8] = b"agnes-sync-recovery-wrap-key-v1";
const RECOVERY_KEY_PREFIX: &str = "agnes-recovery-key-v1.";
const RECOVERY_BUNDLE_PREFIX: &str = "agnes-recovery-bundle-v1.";
const RECOVERY_KEY_BYTES: usize = 32;
const RECOVERY_SALT_BYTES: usize = 16;
const MAX_KEYSET_JSON_BYTES: usize = 16 * 1024;
const MAX_RECOVERY_BUNDLE_BYTES: usize = 32 * 1024;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SyncMasterKey([u8; MASTER_KEY_BYTES]);

impl SyncMasterKey {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; MASTER_KEY_BYTES];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    fn decode(encoded: &str) -> AppResult<Self> {
        let mut decoded = STANDARD_NO_PAD
            .decode(encoded)
            .map_err(|_| invalid_keyset())?;
        if decoded.len() != MASTER_KEY_BYTES || STANDARD_NO_PAD.encode(&decoded) != encoded {
            decoded.zeroize();
            return Err(invalid_keyset());
        }
        let mut bytes = [0_u8; MASTER_KEY_BYTES];
        bytes.copy_from_slice(&decoded);
        decoded.zeroize();
        Ok(Self(bytes))
    }

    fn encode(&self) -> String {
        STANDARD_NO_PAD.encode(self.0)
    }

    #[cfg(test)]
    fn from_bytes(bytes: [u8; MASTER_KEY_BYTES]) -> Self {
        Self(bytes)
    }
}

pub struct SyncKeyset {
    active_key_version: i64,
    keys: BTreeMap<i64, SyncMasterKey>,
}

impl SyncKeyset {
    pub fn generate_initial() -> Self {
        Self {
            active_key_version: 1,
            keys: BTreeMap::from([(1, SyncMasterKey::generate())]),
        }
    }

    pub fn parse(raw: &str) -> AppResult<Self> {
        if raw.len() > MAX_KEYSET_JSON_BYTES {
            return Err(invalid_keyset());
        }
        let stored: StoredKeyset = serde_json::from_str(raw).map_err(|_| invalid_keyset())?;
        if stored.format_version != KEYSET_FORMAT_VERSION
            || stored.active_key_version <= 0
            || stored.keys.is_empty()
            || stored.keys.len() > MAX_KEY_VERSIONS
        {
            return Err(invalid_keyset());
        }

        let mut keys = BTreeMap::new();
        for mut stored_key in stored.keys {
            if stored_key.version <= 0 || keys.contains_key(&stored_key.version) {
                stored_key.key.zeroize();
                return Err(invalid_keyset());
            }
            let key = SyncMasterKey::decode(&stored_key.key);
            stored_key.key.zeroize();
            keys.insert(stored_key.version, key?);
        }
        if !keys.contains_key(&stored.active_key_version) {
            return Err(invalid_keyset());
        }
        Ok(Self {
            active_key_version: stored.active_key_version,
            keys,
        })
    }

    pub fn serialize(&self) -> AppResult<String> {
        let mut stored = StoredKeyset {
            format_version: KEYSET_FORMAT_VERSION,
            active_key_version: self.active_key_version,
            keys: self
                .keys
                .iter()
                .map(|(version, key)| StoredKey {
                    version: *version,
                    key: key.encode(),
                })
                .collect(),
        };
        let serialized = serde_json::to_string(&stored).map_err(Into::into);
        for key in &mut stored.keys {
            key.key.zeroize();
        }
        serialized
    }

    pub fn active_key_version(&self) -> i64 {
        self.active_key_version
    }

    pub fn active_key(&self) -> &SyncMasterKey {
        self.keys
            .get(&self.active_key_version)
            .expect("validated keyset always contains its active key")
    }

    pub fn key(&self, version: i64) -> Option<&SyncMasterKey> {
        self.keys.get(&version)
    }

    pub fn rotate(&mut self) -> AppResult<i64> {
        if self.keys.len() >= MAX_KEY_VERSIONS {
            return Err(AppError::Other(
                "Sync key rotation requires archiving an old key version first".into(),
            ));
        }
        let next_version = match self.keys.last_key_value() {
            Some((version, _)) => version
                .checked_add(1)
                .ok_or_else(|| AppError::Other("Sync key version is exhausted".into()))?,
            None => 1,
        };
        self.keys.insert(next_version, SyncMasterKey::generate());
        self.active_key_version = next_version;
        Ok(next_version)
    }

    pub fn create_recovery_material(&self) -> AppResult<RecoveryMaterial> {
        let mut recovery_secret = Zeroizing::new([0_u8; RECOVERY_KEY_BYTES]);
        OsRng.fill_bytes(recovery_secret.as_mut());
        let recovery_key = format!(
            "{RECOVERY_KEY_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(recovery_secret.as_ref())
        );
        let mut salt = [0_u8; RECOVERY_SALT_BYTES];
        OsRng.fill_bytes(&mut salt);
        let wrapping_key = derive_recovery_key(recovery_secret.as_ref(), &salt)?;

        let nonce = XNonce::generate();
        let cipher = XChaCha20Poly1305::new((&wrapping_key.0).into());
        let mut plaintext = self.serialize()?;
        let ciphertext = cipher.encrypt(
            &nonce,
            Payload {
                msg: plaintext.as_bytes(),
                aad: RECOVERY_AAD,
            },
        );
        plaintext.zeroize();
        let ciphertext = ciphertext
            .map_err(|_| AppError::Other("Unable to create sync recovery material".into()))?;
        let bundle = StoredRecoveryBundle {
            format_version: 1,
            kdf: "hkdf-sha256".into(),
            cipher: "xchacha20poly1305".into(),
            salt: URL_SAFE_NO_PAD.encode(salt),
            nonce: URL_SAFE_NO_PAD.encode(&nonce[..]),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        };
        let bundle = serde_json::to_vec(&bundle)?;
        Ok(RecoveryMaterial {
            recovery_key,
            recovery_bundle: format!("{RECOVERY_BUNDLE_PREFIX}{}", URL_SAFE_NO_PAD.encode(bundle)),
            active_key_version: self.active_key_version,
        })
    }

    pub fn recover(recovery_key: &str, recovery_bundle: &str) -> AppResult<Self> {
        if recovery_key.len() > 128 || recovery_bundle.len() > MAX_RECOVERY_BUNDLE_BYTES {
            return Err(invalid_recovery_material());
        }
        let encoded_key = recovery_key
            .strip_prefix(RECOVERY_KEY_PREFIX)
            .ok_or_else(invalid_recovery_material)?;
        let recovery_secret = Zeroizing::new(decode_canonical_url(encoded_key)?);
        if recovery_secret.len() != RECOVERY_KEY_BYTES {
            return Err(invalid_recovery_material());
        }
        let encoded_bundle = recovery_bundle
            .strip_prefix(RECOVERY_BUNDLE_PREFIX)
            .ok_or_else(invalid_recovery_material)?;
        let bundle_bytes = decode_canonical_url(encoded_bundle)?;
        let bundle: StoredRecoveryBundle =
            serde_json::from_slice(&bundle_bytes).map_err(|_| invalid_recovery_material())?;
        if bundle.format_version != 1
            || bundle.kdf != "hkdf-sha256"
            || bundle.cipher != "xchacha20poly1305"
        {
            return Err(invalid_recovery_material());
        }
        let salt = decode_canonical_url(&bundle.salt)?;
        let nonce = decode_canonical_url(&bundle.nonce)?;
        let ciphertext = decode_canonical_url(&bundle.ciphertext)?;
        if salt.len() != RECOVERY_SALT_BYTES
            || nonce.len() != NONCE_BYTES
            || ciphertext.len() < TAG_BYTES
            || ciphertext.len() > MAX_KEYSET_JSON_BYTES + TAG_BYTES
        {
            return Err(invalid_recovery_material());
        }
        let wrapping_key = derive_recovery_key(recovery_secret.as_ref(), &salt)?;
        let nonce = XNonce::try_from(nonce.as_slice()).map_err(|_| invalid_recovery_material())?;
        let cipher = XChaCha20Poly1305::new((&wrapping_key.0).into());
        let mut plaintext = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &ciphertext,
                    aad: RECOVERY_AAD,
                },
            )
            .map_err(|_| invalid_recovery_material())?;
        let keyset = std::str::from_utf8(&plaintext)
            .map_err(|_| invalid_recovery_material())
            .and_then(Self::parse);
        plaintext.zeroize();
        keyset
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryMaterial {
    pub recovery_key: String,
    pub recovery_bundle: String,
    pub active_key_version: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredRecoveryBundle {
    format_version: u8,
    kdf: String,
    cipher: String,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredKeyset {
    format_version: u8,
    active_key_version: i64,
    keys: Vec<StoredKey>,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredKey {
    version: i64,
    key: String,
}

#[derive(Debug, Clone)]
pub struct PayloadMetadata<'a> {
    pub protocol_version: u8,
    pub entity_type: &'a str,
    pub entity_id: &'a str,
    pub revision: i64,
    pub hlc: &'a str,
    pub payload_schema_version: i64,
    pub origin_device_id: &'a str,
    pub key_version: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct EncryptedPayload {
    pub payload: Value,
    pub payload_hash: String,
}

pub fn seal_json(
    key: &SyncMasterKey,
    metadata: &PayloadMetadata<'_>,
    value: &Value,
) -> AppResult<EncryptedPayload> {
    seal_json_with_nonce(key, metadata, value, XNonce::generate())
}

pub fn open_json(
    key: &SyncMasterKey,
    metadata: &PayloadMetadata<'_>,
    payload: &Value,
    payload_hash: &str,
) -> AppResult<Value> {
    let aad = encode_aad(metadata)?;
    let encoded = payload
        .as_str()
        .ok_or_else(|| invalid_encrypted_payload("payload must be a base64 string"))?;
    let packed = STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid_encrypted_payload("payload base64 is invalid"))?;
    if STANDARD_NO_PAD.encode(&packed) != encoded || packed.len() < NONCE_BYTES + TAG_BYTES {
        return Err(invalid_encrypted_payload("payload framing is invalid"));
    }
    if sha256_hex(&packed) != payload_hash {
        return Err(invalid_encrypted_payload("payload hash does not match"));
    }
    let (nonce, ciphertext) = packed.split_at(NONCE_BYTES);
    let nonce = XNonce::try_from(nonce)
        .map_err(|_| invalid_encrypted_payload("payload nonce is invalid"))?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let mut plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad: &aad,
            },
        )
        .map_err(|_| invalid_encrypted_payload("payload authentication failed"))?;
    let value = serde_json::from_slice(&plaintext)
        .map_err(|_| invalid_encrypted_payload("plaintext JSON is invalid"));
    plaintext.zeroize();
    value
}

fn seal_json_with_nonce(
    key: &SyncMasterKey,
    metadata: &PayloadMetadata<'_>,
    value: &Value,
    nonce: XNonce,
) -> AppResult<EncryptedPayload> {
    let aad = encode_aad(metadata)?;
    let mut plaintext = serde_json::to_vec(value)?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let ciphertext = cipher.encrypt(
        &nonce,
        Payload {
            msg: &plaintext,
            aad: &aad,
        },
    );
    plaintext.zeroize();
    let ciphertext =
        ciphertext.map_err(|_| AppError::Other("Unable to encrypt sync payload".into()))?;
    let mut packed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
    packed.extend_from_slice(nonce.as_ref());
    packed.extend_from_slice(&ciphertext);
    Ok(EncryptedPayload {
        payload: Value::String(STANDARD_NO_PAD.encode(&packed)),
        payload_hash: sha256_hex(&packed),
    })
}

fn encode_aad(metadata: &PayloadMetadata<'_>) -> AppResult<Vec<u8>> {
    if metadata.protocol_version == 0
        || metadata.entity_type.is_empty()
        || metadata.entity_id.is_empty()
        || metadata.revision <= 0
        || metadata.hlc.is_empty()
        || metadata.payload_schema_version <= 0
        || metadata.origin_device_id.is_empty()
        || metadata.key_version <= 0
    {
        return Err(AppError::Other("Invalid sync payload metadata".into()));
    }

    let mut aad = Vec::with_capacity(192);
    aad.extend_from_slice(AAD_DOMAIN);
    aad.push(metadata.protocol_version);
    append_field(&mut aad, metadata.entity_type.as_bytes())?;
    append_field(&mut aad, metadata.entity_id.as_bytes())?;
    aad.extend_from_slice(&metadata.revision.to_be_bytes());
    append_field(&mut aad, metadata.hlc.as_bytes())?;
    aad.extend_from_slice(&metadata.payload_schema_version.to_be_bytes());
    append_field(&mut aad, metadata.origin_device_id.as_bytes())?;
    append_field(&mut aad, PAYLOAD_ENCODING.as_bytes())?;
    aad.extend_from_slice(&metadata.key_version.to_be_bytes());
    Ok(aad)
}

fn append_field(output: &mut Vec<u8>, value: &[u8]) -> AppResult<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| AppError::Other("Sync payload metadata is too large".into()))?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn invalid_keyset() -> AppError {
    AppError::Other("Stored sync encryption keys are invalid".into())
}

fn invalid_recovery_material() -> AppError {
    AppError::Other("Sync recovery material is invalid or does not match".into())
}

fn derive_recovery_key(secret: &[u8], salt: &[u8]) -> AppResult<SyncMasterKey> {
    let mut key = [0_u8; MASTER_KEY_BYTES];
    Hkdf::<Sha256>::new(Some(salt), secret)
        .expand(RECOVERY_INFO, &mut key)
        .map_err(|_| invalid_recovery_material())?;
    Ok(SyncMasterKey(key))
}

fn decode_canonical_url(encoded: &str) -> AppResult<Vec<u8>> {
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid_recovery_material())?;
    if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
        return Err(invalid_recovery_material());
    }
    Ok(decoded)
}

fn invalid_encrypted_payload(reason: &str) -> AppError {
    AppError::Other(format!("Invalid encrypted sync payload: {reason}"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn metadata<'a>() -> PayloadMetadata<'a> {
        PayloadMetadata {
            protocol_version: 1,
            entity_type: "memory",
            entity_id: "memory-1",
            revision: 7,
            hlc: "1784188800123-0004-device01",
            payload_schema_version: 1,
            origin_device_id: "00000000-0000-4000-8000-000000000001",
            key_version: 3,
        }
    }

    #[test]
    fn keyset_round_trips_rotates_and_rejects_invalid_active_versions() {
        let mut keyset = SyncKeyset {
            active_key_version: 1,
            keys: BTreeMap::from([(1, SyncMasterKey::from_bytes([7; MASTER_KEY_BYTES]))]),
        };
        assert_eq!(keyset.rotate().unwrap(), 2);
        assert!(keyset.key(1).is_some());
        assert!(keyset.key(2).is_some());

        let encoded = keyset.serialize().unwrap();
        let decoded = SyncKeyset::parse(&encoded).unwrap();
        assert_eq!(decoded.active_key_version(), 2);
        assert!(decoded.key(1).is_some());
        assert!(decoded.key(2).is_some());

        let mut invalid: Value = serde_json::from_str(&encoded).unwrap();
        invalid["activeKeyVersion"] = 99.into();
        assert!(SyncKeyset::parse(&invalid.to_string()).is_err());
        let mut invalid: Value = serde_json::from_str(&encoded).unwrap();
        invalid["unknown"] = true.into();
        assert!(SyncKeyset::parse(&invalid.to_string()).is_err());
        let mut invalid: Value = serde_json::from_str(&encoded).unwrap();
        invalid["keys"][1]["version"] = 1.into();
        assert!(SyncKeyset::parse(&invalid.to_string()).is_err());
        let mut invalid: Value = serde_json::from_str(&encoded).unwrap();
        invalid["keys"][0]["key"] = "not-base64".into();
        assert!(SyncKeyset::parse(&invalid.to_string()).is_err());

        let mut exhausted = SyncKeyset {
            active_key_version: i64::MAX,
            keys: BTreeMap::from([(i64::MAX, SyncMasterKey::from_bytes([9; MASTER_KEY_BYTES]))]),
        };
        assert!(exhausted.rotate().is_err());
    }

    #[test]
    fn encrypted_json_round_trips_and_uses_a_random_nonce() {
        let key = SyncMasterKey::from_bytes([7; MASTER_KEY_BYTES]);
        let value = json!({"content": "private memory", "keywords": ["cpp", "game"]});
        let first = seal_json(&key, &metadata(), &value).unwrap();
        let second = seal_json(&key, &metadata(), &value).unwrap();
        assert!(first != second);
        assert_eq!(
            open_json(&key, &metadata(), &first.payload, &first.payload_hash).unwrap(),
            value
        );
        assert_eq!(first.payload_hash.len(), 64);
    }

    #[test]
    fn decryption_rejects_wrong_keys_metadata_hashes_and_ciphertext() {
        let key = SyncMasterKey::from_bytes([7; MASTER_KEY_BYTES]);
        let wrong_key = SyncMasterKey::from_bytes([8; MASTER_KEY_BYTES]);
        let encrypted = seal_json(&key, &metadata(), &json!({"content": "secret"})).unwrap();

        assert!(open_json(
            &wrong_key,
            &metadata(),
            &encrypted.payload,
            &encrypted.payload_hash
        )
        .is_err());
        let mut wrong_metadata = metadata();
        wrong_metadata.entity_id = "memory-2";
        assert!(open_json(
            &key,
            &wrong_metadata,
            &encrypted.payload,
            &encrypted.payload_hash
        )
        .is_err());
        assert!(open_json(&key, &metadata(), &encrypted.payload, &"0".repeat(64)).is_err());

        let mut packed = STANDARD_NO_PAD
            .decode(encrypted.payload.as_str().unwrap())
            .unwrap();
        *packed.last_mut().unwrap() ^= 1;
        let tampered = Value::String(STANDARD_NO_PAD.encode(&packed));
        assert!(open_json(&key, &metadata(), &tampered, &sha256_hex(&packed)).is_err());
    }

    #[test]
    fn aad_and_ciphertext_format_have_a_stable_test_vector() {
        let key = SyncMasterKey::from_bytes([7; MASTER_KEY_BYTES]);
        let encrypted = seal_json_with_nonce(
            &key,
            &metadata(),
            &json!({"content": "private memory"}),
            [9; NONCE_BYTES].into(),
        )
        .unwrap();
        assert_eq!(
            encrypted.payload.as_str().unwrap(),
            "CQkJCQkJCQkJCQkJCQkJCQkJCQkJCQkJxVCB5RLeHAyAMDIeGAFunUuH0rotBPCO1PQ6IZFo5qqHKBwKuLekgSVycXw"
        );
        assert_eq!(
            encrypted.payload_hash,
            "1cd7e8c0978997fe41774162553a5321e1335df5a38919f08712e960bbb3f15c"
        );
        assert_eq!(
            open_json(
                &key,
                &metadata(),
                &encrypted.payload,
                &encrypted.payload_hash
            )
            .unwrap(),
            json!({"content": "private memory"})
        );
    }

    #[test]
    fn recovery_material_round_trips_the_full_keyset_and_rejects_mismatches() {
        let mut keyset = SyncKeyset {
            active_key_version: 1,
            keys: BTreeMap::from([(1, SyncMasterKey::from_bytes([7; MASTER_KEY_BYTES]))]),
        };
        keyset.rotate().unwrap();
        let expected = keyset.serialize().unwrap();
        let material = keyset.create_recovery_material().unwrap();
        let recovered =
            SyncKeyset::recover(&material.recovery_key, &material.recovery_bundle).unwrap();
        assert_eq!(recovered.serialize().unwrap(), expected);
        assert_eq!(material.active_key_version, 2);
        assert!(!material.recovery_bundle.contains(&expected));

        let other = keyset.create_recovery_material().unwrap();
        assert!(SyncKeyset::recover(&other.recovery_key, &material.recovery_bundle).is_err());
        let mut tampered = material.recovery_bundle;
        let replacement = if tampered.ends_with('A') { "B" } else { "A" };
        tampered.replace_range(tampered.len() - 1.., replacement);
        assert!(SyncKeyset::recover(&material.recovery_key, &tampered).is_err());
    }
}
