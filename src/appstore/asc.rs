//! App Store Connect REST API client (ES256 JWT auth).
//!
//! API key setup (once): App Store Connect → Users and Access → Integrations →
//! App Store Connect API → create a key with the "App Manager" role, download
//! AuthKey_<KEYID>.p8 into ~/.appstoreconnect/private_keys/, and export
//! APP_STORE_API_KEY_ID and APP_STORE_API_ISSUER_ID in your shell profile.

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BASE: &str = "https://api.appstoreconnect.apple.com";

pub struct Credentials {
    pub key_id: String,
    pub issuer_id: String,
    pub key_file: PathBuf,
}

impl Credentials {
    /// From APP_STORE_API_KEY_ID / APP_STORE_API_ISSUER_ID; the key file is
    /// ~/.appstoreconnect/private_keys/AuthKey_<KEYID>.p8 (also where altool
    /// looks for it).
    pub fn from_env() -> Result<Self> {
        let var = |n: &str| match std::env::var(n) {
            Ok(v) if !v.is_empty() => Ok(v),
            _ => bail!("{n} is not set (see `release ios --help` / README for the API key setup)"),
        };
        let key_id = var("APP_STORE_API_KEY_ID")?;
        let issuer_id = var("APP_STORE_API_ISSUER_ID")?;
        let home = std::env::var("HOME").context("HOME not set")?;
        let key_file = PathBuf::from(home).join(format!(".appstoreconnect/private_keys/AuthKey_{key_id}.p8"));
        Ok(Self { key_id, issuer_id, key_file })
    }

    pub fn require_key_file(&self) -> Result<()> {
        if std::fs::metadata(&self.key_file).is_err() {
            bail!(
                "API key file not found or not readable: {}\n  Download the .p8 file from App Store Connect and place it there.",
                self.key_file.display()
            );
        }
        Ok(())
    }
}

pub struct Asc {
    creds: Credentials,
    key: jsonwebtoken::EncodingKey,
    http: Client,
}

impl Asc {
    pub fn new(creds: Credentials) -> Result<Self> {
        let pem = std::fs::read(&creds.key_file)
            .with_context(|| format!("cannot read {}", creds.key_file.display()))?;
        let key = jsonwebtoken::EncodingKey::from_ec_pem(&pem).context("invalid .p8 key")?;
        let http = Client::builder().timeout(Duration::from_secs(120)).build()?;
        Ok(Self { creds, key, http })
    }

    /// Tokens live 20 minutes; a fresh one per request sidesteps expiry
    /// during long screenshot uploads.
    fn token(&self) -> Result<String> {
        make_token(&self.creds.key_id, &self.creds.issuer_id, &self.key)
    }

    fn send(&self, req: RequestBuilder) -> Result<Response> {
        let r = req.bearer_auth(self.token()?).send()?;
        if r.status().is_success() {
            return Ok(r);
        }
        let status = r.status();
        let url = r.url().to_string();
        let body = r.text().unwrap_or_default();
        let detail = serde_json::from_str::<Value>(&body)
            .map(|v| serde_json::to_string_pretty(&v).unwrap_or(body.clone()))
            .unwrap_or(body);
        bail!("ASC API error {status} {url}:\n{detail}");
    }

    fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        Ok(self.send(self.http.get(format!("{BASE}{path}")).query(query))?.json()?)
    }

    fn post(&self, path: &str, body: Value) -> Result<Value> {
        Ok(self.send(self.http.post(format!("{BASE}{path}")).json(&body))?.json()?)
    }

    fn patch(&self, path: &str, body: Value) -> Result<()> {
        self.send(self.http.patch(format!("{BASE}{path}")).json(&body))?;
        Ok(())
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.send(self.http.delete(format!("{BASE}{path}")))?;
        Ok(())
    }

    pub fn app_id(&self, bundle_id: &str) -> Result<String> {
        let data = self.get("/v1/apps", &[("filter[bundleId]", bundle_id)])?;
        first_id(&data).with_context(|| format!("no app found for bundle ID {bundle_id}"))
    }

    pub fn find_version(&self, app_id: &str, version: &str) -> Result<Option<String>> {
        let data = self.get(
            &format!("/v1/apps/{app_id}/appStoreVersions"),
            &[("filter[platform]", "IOS"), ("filter[versionString]", version)],
        )?;
        Ok(first_id(&data))
    }

    pub fn create_version(&self, app_id: &str, version: &str) -> Result<String> {
        let data = self.post(
            "/v1/appStoreVersions",
            json!({"data": {
                "type": "appStoreVersions",
                "attributes": {"platform": "IOS", "versionString": version, "releaseType": "AFTER_APPROVAL"},
                "relationships": {"app": {"data": {"type": "apps", "id": app_id}}},
            }}),
        )?;
        str_at(&data, &["data", "id"])
    }

    pub fn localization(&self, version_id: &str, locale: &str) -> Result<String> {
        let data = self.get(
            &format!("/v1/appStoreVersions/{version_id}/appStoreVersionLocalizations"),
            &[("filter[locale]", locale)],
        )?;
        first_id(&data).with_context(|| format!("no localization found for locale {locale}"))
    }

    /// `attrs`: (API attribute name, value) pairs, e.g. ("whatsNew", "…").
    pub fn update_localization(&self, loc_id: &str, attrs: &[(&str, String)]) -> Result<()> {
        let attributes: serde_json::Map<String, Value> =
            attrs.iter().map(|(k, v)| (k.to_string(), Value::String(v.clone()))).collect();
        self.patch(
            &format!("/v1/appStoreVersionLocalizations/{loc_id}"),
            json!({"data": {"type": "appStoreVersionLocalizations", "id": loc_id, "attributes": attributes}}),
        )
    }

    /// The newest processed (VALID) build for `version`.
    pub fn find_build(&self, app_id: &str, version: &str) -> Result<Option<String>> {
        let data = self.get(
            "/v1/builds",
            &[
                ("filter[app]", app_id),
                ("filter[preReleaseVersion.version]", version),
                ("filter[processingState]", "VALID"),
                ("sort", "-uploadedDate"),
                ("limit", "1"),
            ],
        )?;
        Ok(first_id(&data))
    }

    pub fn build_number(&self, build_id: &str) -> Result<String> {
        let data = self.get(&format!("/v1/builds/{build_id}"), &[])?;
        str_at(&data, &["data", "attributes", "version"])
    }

    pub fn associate_build(&self, version_id: &str, build_id: &str) -> Result<()> {
        self.patch(
            &format!("/v1/appStoreVersions/{version_id}/relationships/build"),
            json!({"data": {"type": "builds", "id": build_id}}),
        )
    }

    /// Replace the screenshot set for `display_type` with `files`, in order.
    pub fn replace_screenshots(&self, loc_id: &str, display_type: &str, files: &[PathBuf]) -> Result<()> {
        let set_id = self.screenshot_set(loc_id, display_type)?;
        let existing = self.get(&format!("/v1/appScreenshotSets/{set_id}/appScreenshots"), &[])?;
        for item in existing["data"].as_array().into_iter().flatten() {
            if let Some(id) = item["id"].as_str() {
                self.delete(&format!("/v1/appScreenshots/{id}"))?;
            }
        }
        for f in files {
            self.upload_screenshot(&set_id, f)?;
            println!("  uploaded {} → {display_type}", f.file_name().unwrap_or_default().to_string_lossy());
        }
        Ok(())
    }

    fn screenshot_set(&self, loc_id: &str, display_type: &str) -> Result<String> {
        let data = self.get(&format!("/v1/appStoreVersionLocalizations/{loc_id}/appScreenshotSets"), &[])?;
        for item in data["data"].as_array().into_iter().flatten() {
            if item["attributes"]["screenshotDisplayType"] == display_type {
                return str_at(item, &["id"]);
            }
        }
        let data = self.post(
            "/v1/appScreenshotSets",
            json!({"data": {
                "type": "appScreenshotSets",
                "attributes": {"screenshotDisplayType": display_type},
                "relationships": {"appStoreVersionLocalization": {"data": {"type": "appStoreVersionLocalizations", "id": loc_id}}},
            }}),
        )?;
        str_at(&data, &["data", "id"])
    }

    /// Reserve an upload slot, PUT the bytes to Apple's CDN per the returned
    /// upload operations, then confirm with the file's MD5.
    fn upload_screenshot(&self, set_id: &str, path: &Path) -> Result<()> {
        let bytes = std::fs::read(path)?;
        let name = path.file_name().context("file name")?.to_string_lossy().into_owned();
        let reservation = self.post(
            "/v1/appScreenshots",
            json!({"data": {
                "type": "appScreenshots",
                "attributes": {"fileSize": bytes.len(), "fileName": name},
                "relationships": {"appScreenshotSet": {"data": {"type": "appScreenshotSets", "id": set_id}}},
            }}),
        )?;
        let id = str_at(&reservation, &["data", "id"])?;
        for op in reservation["data"]["attributes"]["uploadOperations"].as_array().into_iter().flatten() {
            let url = op["url"].as_str().context("upload operation without url")?;
            let offset = op["offset"].as_u64().unwrap_or(0) as usize;
            let length = op["length"].as_u64().map(|l| l as usize).unwrap_or(bytes.len());
            let chunk = bytes.get(offset..offset + length).context("upload operation out of range")?;
            let content_type = op["requestHeaders"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case("content-type")))
                .and_then(|h| h["value"].as_str())
                .unwrap_or("image/png");
            let r = self
                .http
                .put(url)
                .header("Content-Type", content_type)
                .body(chunk.to_vec())
                .send()?;
            if !r.status().is_success() {
                bail!("screenshot upload error {}: {}", r.status(), r.text().unwrap_or_default());
            }
        }
        self.patch(
            &format!("/v1/appScreenshots/{id}"),
            json!({"data": {
                "type": "appScreenshots", "id": id,
                "attributes": {"uploaded": true, "sourceFileChecksum": format!("{:x}", md5::compute(&bytes))},
            }}),
        )
    }
}

fn make_token(key_id: &str, issuer_id: &str, key: &jsonwebtoken::EncodingKey) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
    header.kid = Some(key_id.to_string());
    let claims = json!({"iss": issuer_id, "iat": now, "exp": now + 1200, "aud": "appstoreconnect-v1"});
    Ok(jsonwebtoken::encode(&header, &claims, key)?)
}

fn first_id(data: &Value) -> Option<String> {
    data["data"].as_array()?.first()?["id"].as_str().map(String::from)
}

fn str_at(v: &Value, path: &[&str]) -> Result<String> {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_str()
        .map(String::from)
        .with_context(|| format!("unexpected ASC response: no {}", path.join(".")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    /// Throwaway P-256 key, generated for this test only.
    const TEST_KEY: &str = include_str!("../../tests/fixtures/test-es256.p8");

    #[test]
    fn token_has_asc_claims() {
        let key = jsonwebtoken::EncodingKey::from_ec_pem(TEST_KEY.as_bytes()).unwrap();
        let token = make_token("KID123", "issuer-uuid", &key).unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let dec = |s: &str| -> Value {
            serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).unwrap()).unwrap()
        };
        let header = dec(parts[0]);
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "KID123");
        let claims = dec(parts[1]);
        assert_eq!(claims["iss"], "issuer-uuid");
        assert_eq!(claims["aud"], "appstoreconnect-v1");
        assert_eq!(claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(), 1200);
    }

    #[test]
    fn json_helpers() {
        let v = json!({"data": [{"id": "a"}, {"id": "b"}]});
        assert_eq!(first_id(&v).as_deref(), Some("a"));
        assert_eq!(first_id(&json!({"data": []})), None);
        assert_eq!(str_at(&json!({"data": {"id": "x"}}), &["data", "id"]).unwrap(), "x");
        assert!(str_at(&json!({}), &["data", "id"]).is_err());
    }
}
