//! The device credential and the signed device routes (`/api/device/*`), the same design as the Android app
//! (V1.1-DESIGN.md): a per-device P-256 key signs every request, and the device-tier Access service token gets it
//! past Cloudflare Access. Both live in the macOS Keychain, never on disk in clear.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{DerSignature, SigningKey};
use p256::pkcs8::EncodePublicKey;
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::logic::{signed_string, Needs, Work, ORIGIN};

const SERVICE: &str = "cool.zem.board.mac";
const ACCOUNT: &str = "device";

/// Everything the app needs to call the device routes. Serialized into one Keychain item.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credential {
    pub device_id: String,
    /// The P-256 private scalar, base64url.
    key: String,
    pub instance_id: String,
    client_id: String,
    client_secret: String,
}

impl Credential {
    fn signing_key(&self) -> Option<SigningKey> {
        SigningKey::from_slice(&B64.decode(&self.key).ok()?).ok()
    }
}

#[cfg(target_os = "macos")]
mod store {
    use security_framework::passwords::{delete_generic_password, get_generic_password, set_generic_password};
    pub fn get(service: &str, account: &str) -> Option<Vec<u8>> {
        get_generic_password(service, account).ok()
    }
    pub fn set(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
        set_generic_password(service, account, value).map_err(|e| e.to_string())
    }
    pub fn delete(service: &str, account: &str) {
        let _ = delete_generic_password(service, account);
    }
}

#[cfg(not(target_os = "macos"))]
mod store {
    // Only for unit tests off a Mac: an in-memory stand-in.
    use std::sync::Mutex;
    static ITEM: Mutex<Option<Vec<u8>>> = Mutex::new(None);
    pub fn get(_: &str, _: &str) -> Option<Vec<u8>> {
        ITEM.lock().unwrap().clone()
    }
    pub fn set(_: &str, _: &str, value: &[u8]) -> Result<(), String> {
        *ITEM.lock().unwrap() = Some(value.to_vec());
        Ok(())
    }
    pub fn delete(_: &str, _: &str) {
        *ITEM.lock().unwrap() = None;
    }
}

pub fn load() -> Option<Credential> {
    serde_json::from_slice(&store::get(SERVICE, ACCOUNT)?).ok()
}

pub fn forget() {
    store::delete(SERVICE, ACCOUNT);
}

/// The app instance id survives a revoke, so a re-pair replaces this Mac's old record instead of adding one.
fn instance_id() -> String {
    if let Some(id) = store::get(SERVICE, "instance").and_then(|v| String::from_utf8(v).ok()) {
        return id;
    }
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    let id = B64.encode(bytes);
    let _ = store::set(SERVICE, "instance", id.as_bytes());
    id
}

#[derive(Debug)]
pub enum ApiError {
    /// The Board said this device is gone (revoked or expired): pair again.
    Revoked,
    NotPaired,
    Status(u16, String),
    Network(#[allow(dead_code)] String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Revoked => write!(f, "This Mac was disconnected from the Board"),
            ApiError::NotPaired => write!(f, "This Mac isn't connected to the Board yet"),
            ApiError::Status(409, _) => write!(f, "Already answered"),
            ApiError::Status(429, _) => write!(f, "Too many requests; try again in a minute"),
            ApiError::Status(code, message) if !message.is_empty() => write!(f, "{message} ({code})"),
            ApiError::Status(code, _) => write!(f, "The Board answered {code}"),
            ApiError::Network(_) => write!(f, "Can't reach the Board"),
        }
    }
}

pub enum Answer {
    Option(String),
    Text(String),
}

pub struct Api {
    http: reqwest::Client,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

impl Api {
    pub fn new(user_agent: &str) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(user_agent)
            // A redirect here is Cloudflare Access sending us to its login page: never follow it.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .https_only(true)
            .build()
            .expect("http client");
        Api { http }
    }

    async fn signed(&self, cred: &Credential, method: reqwest::Method, path: &str, body: Option<String>) -> Result<String, ApiError> {
        let key = cred.signing_key().ok_or(ApiError::NotPaired)?;
        let body = body.unwrap_or_default();
        let time = now_ms().to_string();
        let mut nonce = [0u8; 18];
        rand::thread_rng().fill_bytes(&mut nonce);
        let nonce = B64.encode(nonce);
        let signature: DerSignature = key.sign(signed_string(method.as_str(), path, &time, &nonce, body.as_bytes()).as_bytes());
        let mut request = self
            .http
            .request(method, format!("{ORIGIN}{path}"))
            .header("CF-Access-Client-Id", &cred.client_id)
            .header("CF-Access-Client-Secret", &cred.client_secret)
            .header("Board-Device", &cred.device_id)
            .header("Board-Time", time)
            .header("Board-Nonce", nonce)
            .header("Board-Signature", B64.encode(signature.as_bytes()))
            .header("Accept", "application/json");
        if !body.is_empty() {
            request = request.header("Content-Type", "application/json").body(body);
        }
        let response = request.send().await.map_err(|e| ApiError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        if (200..300).contains(&status) {
            return Ok(text);
        }
        let parsed: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        // Only this code means the device is gone; clock, signature or replay refusals are transient (devices.mjs).
        if status == 401 && parsed["code"] == "device-revoked" {
            return Err(ApiError::Revoked);
        }
        Err(ApiError::Status(status, parsed["error"].as_str().unwrap_or("").to_string()))
    }

    pub async fn needs(&self, cred: &Credential) -> Result<Needs, ApiError> {
        let text = self.signed(cred, reqwest::Method::GET, "/api/device/needs", None).await?;
        serde_json::from_str(&text).map_err(|e| ApiError::Network(e.to_string()))
    }

    pub async fn work(&self, cred: &Credential) -> Result<Work, ApiError> {
        let text = self.signed(cred, reqwest::Method::GET, "/api/device/work", None).await?;
        serde_json::from_str(&text).map_err(|e| ApiError::Network(e.to_string()))
    }

    pub async fn set_work(&self, cred: &Credential, on: bool) -> Result<Work, ApiError> {
        let body = serde_json::json!({ "on": on }).to_string();
        let text = self.signed(cred, reqwest::Method::POST, "/api/device/work", Some(body)).await?;
        serde_json::from_str(&text).map_err(|e| ApiError::Network(e.to_string()))
    }

    pub async fn answer(&self, cred: &Credential, card: &str, answer: Answer) -> Result<(), ApiError> {
        let body = match answer {
            Answer::Option(option) => serde_json::json!({ "cardId": card, "option": option }),
            Answer::Text(text) => serde_json::json!({ "cardId": card, "text": text }),
        };
        self.signed(cred, reqwest::Method::POST, "/api/device/answer", Some(body.to_string())).await.map(|_| ())
    }

    /// Redeems a one-time code from the signed-in Board page: a fresh key for this Mac, registered through Access.
    pub async fn pair(&self, code: &str, client_id: &str, client_secret: &str, name: &str) -> Result<Credential, ApiError> {
        let key = SigningKey::random(&mut rand::thread_rng());
        let public = key.verifying_key().to_public_key_der().map_err(|e| ApiError::Network(e.to_string()))?;
        let instance_id = instance_id();
        let body = serde_json::json!({ "code": code, "name": name, "publicKey": B64.encode(public.as_bytes()), "instanceId": instance_id });
        let response = self
            .http
            .post(format!("{ORIGIN}/api/device/pair"))
            .header("CF-Access-Client-Id", client_id)
            .header("CF-Access-Client-Secret", client_secret)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;
        let status = response.status().as_u16();
        let parsed: serde_json::Value = serde_json::from_str(&response.text().await.unwrap_or_default()).unwrap_or_default();
        if status != 201 {
            return Err(ApiError::Status(status, parsed["error"].as_str().unwrap_or("").to_string()));
        }
        let device_id = parsed["deviceId"].as_str().ok_or(ApiError::Status(status, "no device id".into()))?.to_string();
        let cred = Credential {
            device_id,
            key: B64.encode(key.to_bytes()),
            instance_id,
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
        };
        store::set(SERVICE, ACCOUNT, &serde_json::to_vec(&cred).unwrap()).map_err(ApiError::Network)?;
        Ok(cred)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::ecdsa::VerifyingKey;

    #[test]
    fn key_round_trips_and_signs_der() {
        let key = SigningKey::random(&mut rand::thread_rng());
        let cred = Credential { device_id: "d".into(), key: B64.encode(key.to_bytes()), instance_id: "i".into(), client_id: "c".into(), client_secret: "s".into() };
        let back = cred.signing_key().unwrap();
        let message = signed_string("GET", "/api/device/needs", "1", "n", b"");
        let signature: DerSignature = back.sign(message.as_bytes());
        // DER (what node's verify with dsaEncoding 'der' expects) starts with a SEQUENCE tag.
        assert_eq!(signature.as_bytes()[0], 0x30);
        let verifying: &VerifyingKey = key.verifying_key();
        assert!(verifying.verify(message.as_bytes(), &signature).is_ok());
    }
}
