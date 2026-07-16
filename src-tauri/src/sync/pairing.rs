use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chacha20poly1305::{
    aead::{Aead, Generate, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{AppError, AppResult};
use crate::sync::crypto::SyncKeyset;

const PAIRING_CODE_PREFIX: &str = "agnes-pair-v1.";
const PAIRING_SECRET_BYTES: usize = 32;
const PAIRING_KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 24;
const TAG_BYTES: usize = 16;
const PAIRING_INFO: &[u8] = b"agnes-sync-pairing-wrap-key-v1";
const PROOF_AAD: &[u8] = b"agnes-sync-pairing-responder-proof-v1\0";
const TRANSFER_AAD: &[u8] = b"agnes-sync-pairing-transfer-v1\0";
const INITIATOR_ID_PREFIX: &[u8] = b"agnes-sync-pairing-initiator-v1:";
const RESPONDER_ID_PREFIX: &[u8] = b"agnes-sync-pairing-responder-v1:";
const DEVICE_TOKEN_PREFIX: &str = "agnes-device-token-v1.";
const MAX_PAIRING_CODE_BYTES: usize = 1_024;
const MAX_OPAQUE_BYTES: usize = 32 * 1_024;

pub type PairingExchange = Spake2<Ed25519Group>;

#[derive(Serialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase")]
pub struct PairingInvite {
    pub session_id: String,
    pub pairing_code: String,
    pub expires_at: i64,
}

pub struct StartedInitiator {
    pub exchange: PairingExchange,
    pub initiator_message: String,
    pub pairing_code: String,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct PairingKey([u8; PAIRING_KEY_BYTES]);

pub struct ResponderExchange {
    pub key: PairingKey,
    pub responder_message: String,
    pub responder_proof: String,
}

#[derive(Clone)]
pub struct PairingDevice {
    pub device_id: String,
    pub device_name: String,
    pub platform: Option<String>,
}

#[derive(Clone)]
pub struct PreparedPairingTransfer {
    pub device_id: String,
    pub credential_fingerprint: String,
    pub transfer_bundle: String,
}

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct OpenedPairingTransfer {
    pub bearer_token: String,
    pub keyset_json: String,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPairingCode {
    format_version: u8,
    session_id: String,
    secret: String,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairingProof {
    format_version: u8,
    session_id: String,
    device_id: String,
    device_name: String,
    platform: Option<String>,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PairingTransfer {
    format_version: u8,
    session_id: String,
    device_id: String,
    bearer_token: String,
    keyset_json: String,
}

pub fn start_initiator(session_id: &str) -> AppResult<StartedInitiator> {
    validate_session_id(session_id)?;
    let mut secret = Zeroizing::new([0_u8; PAIRING_SECRET_BYTES]);
    OsRng.fill_bytes(secret.as_mut());
    let (initiator_id, responder_id) = pairing_identities(session_id);
    let (exchange, message) = Spake2::<Ed25519Group>::start_a(
        &Password::new(secret.as_slice()),
        &Identity::new(&initiator_id),
        &Identity::new(&responder_id),
    );
    let mut code = StoredPairingCode {
        format_version: 1,
        session_id: session_id.to_string(),
        secret: URL_SAFE_NO_PAD.encode(secret.as_ref()),
    };
    let encoded = Zeroizing::new(serde_json::to_vec(&code)?);
    code.zeroize();
    Ok(StartedInitiator {
        exchange,
        initiator_message: URL_SAFE_NO_PAD.encode(message),
        pairing_code: format!(
            "{PAIRING_CODE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(encoded.as_slice())
        ),
    })
}

pub fn start_responder(
    pairing_code: &str,
    initiator_message: &str,
    device: &PairingDevice,
) -> AppResult<(String, ResponderExchange)> {
    validate_device(device)?;
    let (session_id, secret) = parse_pairing_code(pairing_code)?;
    let inbound = decode_opaque(initiator_message, 256)?;
    let (initiator_id, responder_id) = pairing_identities(&session_id);
    let (exchange, outbound) = Spake2::<Ed25519Group>::start_b(
        &Password::new(secret.as_slice()),
        &Identity::new(&initiator_id),
        &Identity::new(&responder_id),
    );
    let key = finish_exchange(exchange, &inbound)?;
    let mut proof = PairingProof {
        format_version: 1,
        session_id: session_id.clone(),
        device_id: device.device_id.clone(),
        device_name: device.device_name.clone(),
        platform: device.platform.clone(),
    };
    let responder_proof = seal_struct(&key, &proof, PROOF_AAD)?;
    proof.zeroize();
    Ok((
        session_id,
        ResponderExchange {
            key,
            responder_message: URL_SAFE_NO_PAD.encode(outbound),
            responder_proof,
        },
    ))
}

pub fn pairing_session_id(pairing_code: &str) -> AppResult<String> {
    parse_pairing_code(pairing_code).map(|(session_id, _)| session_id)
}

pub fn finish_initiator(
    exchange: PairingExchange,
    session_id: &str,
    responder_message: &str,
    responder_proof: &str,
    expected_device: &PairingDevice,
) -> AppResult<PairingKey> {
    validate_session_id(session_id)?;
    validate_device(expected_device)?;
    let inbound = decode_opaque(responder_message, 256)?;
    let key = finish_exchange(exchange, &inbound)?;
    let proof: PairingProof = open_struct(&key, responder_proof, PROOF_AAD)?;
    if proof.format_version != 1
        || proof.session_id != session_id
        || proof.device_id != expected_device.device_id
        || proof.device_name != expected_device.device_name
        || proof.platform != expected_device.platform
    {
        return Err(invalid_pairing());
    }
    Ok(key)
}

pub fn prepare_transfer(
    key: &PairingKey,
    session_id: &str,
    device_id: &str,
    keyset: &SyncKeyset,
) -> AppResult<PreparedPairingTransfer> {
    validate_session_id(session_id)?;
    if uuid::Uuid::parse_str(device_id).is_err() {
        return Err(invalid_pairing());
    }
    let mut token_bytes = Zeroizing::new([0_u8; PAIRING_SECRET_BYTES]);
    OsRng.fill_bytes(token_bytes.as_mut());
    let bearer_token = format!(
        "{DEVICE_TOKEN_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(token_bytes.as_ref())
    );
    let mut transfer = PairingTransfer {
        format_version: 1,
        session_id: session_id.to_string(),
        device_id: device_id.to_string(),
        bearer_token,
        keyset_json: keyset.serialize()?,
    };
    let credential_fingerprint = sha256_hex(transfer.bearer_token.as_bytes());
    let transfer_bundle = seal_struct(key, &transfer, TRANSFER_AAD)?;
    transfer.zeroize();
    Ok(PreparedPairingTransfer {
        device_id: device_id.to_string(),
        credential_fingerprint,
        transfer_bundle,
    })
}

pub fn open_transfer(
    key: &PairingKey,
    session_id: &str,
    device_id: &str,
    transfer_bundle: &str,
) -> AppResult<(SyncKeyset, OpenedPairingTransfer)> {
    let transfer: PairingTransfer = open_struct(key, transfer_bundle, TRANSFER_AAD)?;
    if transfer.format_version != 1
        || transfer.session_id != session_id
        || transfer.device_id != device_id
        || !transfer.bearer_token.starts_with(DEVICE_TOKEN_PREFIX)
    {
        return Err(invalid_pairing());
    }
    let keyset = SyncKeyset::parse(&transfer.keyset_json)?;
    Ok((
        keyset,
        OpenedPairingTransfer {
            bearer_token: transfer.bearer_token.clone(),
            keyset_json: transfer.keyset_json.clone(),
        },
    ))
}

fn parse_pairing_code(pairing_code: &str) -> AppResult<(String, Zeroizing<Vec<u8>>)> {
    if pairing_code.len() > MAX_PAIRING_CODE_BYTES {
        return Err(invalid_pairing());
    }
    let encoded = pairing_code
        .strip_prefix(PAIRING_CODE_PREFIX)
        .ok_or_else(invalid_pairing)?;
    let decoded = Zeroizing::new(decode_opaque(encoded, MAX_PAIRING_CODE_BYTES)?);
    let code: StoredPairingCode =
        serde_json::from_slice(decoded.as_slice()).map_err(|_| invalid_pairing())?;
    validate_session_id(&code.session_id)?;
    if code.format_version != 1 {
        return Err(invalid_pairing());
    }
    let secret = Zeroizing::new(decode_opaque(&code.secret, 128)?);
    if secret.len() != PAIRING_SECRET_BYTES {
        return Err(invalid_pairing());
    }
    Ok((code.session_id.clone(), secret))
}

fn finish_exchange(exchange: PairingExchange, inbound: &[u8]) -> AppResult<PairingKey> {
    let mut shared = Zeroizing::new(exchange.finish(inbound).map_err(|_| invalid_pairing())?);
    let mut key = [0_u8; PAIRING_KEY_BYTES];
    Hkdf::<Sha256>::new(None, shared.as_ref())
        .expand(PAIRING_INFO, &mut key)
        .map_err(|_| invalid_pairing())?;
    shared.zeroize();
    Ok(PairingKey(key))
}

fn seal_struct<T: Serialize>(key: &PairingKey, value: &T, aad: &[u8]) -> AppResult<String> {
    let nonce = XNonce::generate();
    let mut plaintext = serde_json::to_vec(value)?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let ciphertext = cipher.encrypt(
        &nonce,
        Payload {
            msg: &plaintext,
            aad,
        },
    );
    plaintext.zeroize();
    let ciphertext = ciphertext.map_err(|_| invalid_pairing())?;
    let mut packed = Vec::with_capacity(NONCE_BYTES + ciphertext.len());
    packed.extend_from_slice(nonce.as_ref());
    packed.extend_from_slice(&ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(packed))
}

fn open_struct<T: for<'de> Deserialize<'de>>(
    key: &PairingKey,
    encoded: &str,
    aad: &[u8],
) -> AppResult<T> {
    let packed = decode_opaque(encoded, MAX_OPAQUE_BYTES)?;
    if packed.len() < NONCE_BYTES + TAG_BYTES {
        return Err(invalid_pairing());
    }
    let (nonce, ciphertext) = packed.split_at(NONCE_BYTES);
    let nonce = XNonce::try_from(nonce).map_err(|_| invalid_pairing())?;
    let cipher = XChaCha20Poly1305::new((&key.0).into());
    let mut plaintext = cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| invalid_pairing())?;
    let value = serde_json::from_slice(&plaintext).map_err(|_| invalid_pairing());
    plaintext.zeroize();
    value
}

fn decode_opaque(encoded: &str, maximum: usize) -> AppResult<Vec<u8>> {
    if encoded.is_empty() || encoded.len() > maximum {
        return Err(invalid_pairing());
    }
    let decoded = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid_pairing())?;
    if URL_SAFE_NO_PAD.encode(&decoded) != encoded {
        return Err(invalid_pairing());
    }
    Ok(decoded)
}

fn pairing_identities(session_id: &str) -> (Vec<u8>, Vec<u8>) {
    let mut initiator = INITIATOR_ID_PREFIX.to_vec();
    initiator.extend_from_slice(session_id.as_bytes());
    let mut responder = RESPONDER_ID_PREFIX.to_vec();
    responder.extend_from_slice(session_id.as_bytes());
    (initiator, responder)
}

fn validate_session_id(session_id: &str) -> AppResult<()> {
    uuid::Uuid::parse_str(session_id)
        .map(|_| ())
        .map_err(|_| invalid_pairing())
}

fn validate_device(device: &PairingDevice) -> AppResult<()> {
    if uuid::Uuid::parse_str(&device.device_id).is_err()
        || device.device_name.trim().is_empty()
        || device.device_name.len() > 80
        || device
            .platform
            .as_ref()
            .is_some_and(|value| value.trim().is_empty() || value.len() > 40)
    {
        return Err(invalid_pairing());
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid_pairing() -> AppError {
    AppError::Other("Pairing material is invalid or does not match".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "30000000-0000-4000-8000-000000000001";
    const DEVICE_ID: &str = "00000000-0000-4000-8000-000000000004";

    fn device() -> PairingDevice {
        PairingDevice {
            device_id: DEVICE_ID.into(),
            device_name: "New device".into(),
            platform: Some("linux".into()),
        }
    }

    #[test]
    fn spake2_pairing_authenticates_and_wraps_the_full_keyset_and_token() {
        let initiator = start_initiator(SESSION_ID).unwrap();
        let (session_id, responder) = start_responder(
            &initiator.pairing_code,
            &initiator.initiator_message,
            &device(),
        )
        .unwrap();
        assert_eq!(session_id, SESSION_ID);
        let initiator_key = finish_initiator(
            initiator.exchange,
            SESSION_ID,
            &responder.responder_message,
            &responder.responder_proof,
            &device(),
        )
        .unwrap();
        let mut keyset = SyncKeyset::generate_initial();
        keyset.rotate().unwrap();
        let expected = keyset.serialize().unwrap();
        let transfer = prepare_transfer(&initiator_key, SESSION_ID, DEVICE_ID, &keyset).unwrap();
        let (opened_keyset, opened) = open_transfer(
            &responder.key,
            SESSION_ID,
            DEVICE_ID,
            &transfer.transfer_bundle,
        )
        .unwrap();
        assert_eq!(opened_keyset.serialize().unwrap(), expected);
        assert_eq!(opened.keyset_json, expected);
        assert!(opened.bearer_token.starts_with(DEVICE_TOKEN_PREFIX));
        assert_eq!(
            transfer.credential_fingerprint,
            sha256_hex(opened.bearer_token.as_bytes())
        );
        assert!(!transfer.transfer_bundle.contains(&expected));
        assert!(!transfer.transfer_bundle.contains(&opened.bearer_token));
    }

    #[test]
    fn pairing_rejects_wrong_codes_messages_proofs_and_transfer_aad() {
        let first = start_initiator(SESSION_ID).unwrap();
        let second = start_initiator(SESSION_ID).unwrap();
        let (_, responder) =
            start_responder(&second.pairing_code, &first.initiator_message, &device()).unwrap();
        assert!(finish_initiator(
            first.exchange,
            SESSION_ID,
            &responder.responder_message,
            &responder.responder_proof,
            &device(),
        )
        .is_err());

        let initiator = start_initiator(SESSION_ID).unwrap();
        let (_, responder) = start_responder(
            &initiator.pairing_code,
            &initiator.initiator_message,
            &device(),
        )
        .unwrap();
        let mut other = device();
        other.device_name = "Tampered".into();
        assert!(finish_initiator(
            initiator.exchange,
            SESSION_ID,
            &responder.responder_message,
            &responder.responder_proof,
            &other,
        )
        .is_err());

        let keyset = SyncKeyset::generate_initial();
        let transfer = prepare_transfer(&responder.key, SESSION_ID, DEVICE_ID, &keyset).unwrap();
        assert!(open_transfer(
            &responder.key,
            SESSION_ID,
            "00000000-0000-4000-8000-000000000099",
            &transfer.transfer_bundle,
        )
        .is_err());
    }
}
