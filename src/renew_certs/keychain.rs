//! Developer ID identities in the keychain, via Security.framework: listing,
//! exporting one identity as PKCS#12 (passphrase in memory, not argv),
//! importing, deleting.

use crate::cert::CertInfo;
use anyhow::{Context, Result, bail};
use core_foundation::base::TCFType;
use core_foundation::data::CFData;
use core_foundation::string::CFString;
use security_framework::identity::SecIdentity;
use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
use security_framework::os::macos::import_export::ImportOptions;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework_sys::import_export::{
    SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION, SecItemExport, SecItemImportExportKeyParameters, kSecFormatPKCS12,
};
use releaseit::ui::Ui;
use std::path::PathBuf;

pub struct Identity {
    pub info: CertInfo,
    handle: SecIdentity,
}

/// The keychain to work on: `--keychain`, else the default search list
/// (listing) / the default keychain (import).
pub struct Keychain {
    pub path: Option<PathBuf>,
}

impl Keychain {
    fn open(&self) -> Result<Option<SecKeychain>> {
        self.path
            .as_ref()
            .map(|p| SecKeychain::open(p).with_context(|| format!("cannot open keychain {}", p.display())))
            .transpose()
    }

    /// Every Developer ID Application/Installer identity (certificate with
    /// its private key), oldest expiry first.
    pub fn identities(&self) -> Result<Vec<Identity>> {
        let mut search = ItemSearchOptions::new();
        search.class(ItemClass::identity()).load_refs(true).limit(Limit::All);
        let kc = self.open()?;
        if let Some(kc) = &kc {
            search.keychains(std::slice::from_ref(kc));
        }
        // errSecItemNotFound: no identities at all.
        let results = search.search().unwrap_or_default();
        let mut out = Vec::new();
        for r in results {
            if let SearchResult::Ref(Reference::Identity(handle)) = r {
                let info = CertInfo::from_der(&handle.certificate()?.to_der())?;
                if info.kind.is_some() {
                    out.push(Identity { info, handle });
                }
            }
        }
        out.sort_by_key(|i| i.info.not_after);
        out.dedup_by(|a, b| a.info.sha1 == b.info.sha1);
        Ok(out)
    }

    /// Import a `.p12` (certificate + key).
    pub fn import(&self, p12: &[u8], password: &str) -> Result<()> {
        let kc = match self.open()? {
            Some(kc) => kc,
            None => SecKeychain::default()?,
        };
        ImportOptions::new()
            .filename("identity.p12")
            .pkcs12()
            .passphrase(password)
            .keychain(&kc)
            .import(p12)
            .context("keychain import failed")
    }

    /// Delete an identity (certificate and private key) by SHA-1.
    pub fn delete(&self, ui: &Ui, sha1: &str) -> Result<()> {
        let path = self.path.as_ref().map(|p| p.to_string_lossy());
        let mut args = vec!["delete-identity", "-Z", sha1];
        args.extend(path.as_deref());
        ui.run("security", &args)
    }
}

impl AsRef<CertInfo> for Identity {
    fn as_ref(&self) -> &CertInfo {
        &self.info
    }
}

impl Identity {
    /// Export as PKCS#12 encrypted with `password`. macOS asks once whether
    /// to allow exporting the private key.
    pub fn export(&self, password: &str) -> Result<Vec<u8>> {
        let pass = CFString::new(password);
        let params = SecItemImportExportKeyParameters {
            version: SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION,
            flags: 0,
            passphrase: pass.as_CFTypeRef(),
            alertTitle: std::ptr::null(),
            alertPrompt: std::ptr::null(),
            accessRef: std::ptr::null_mut(),
            keyUsage: std::ptr::null(),
            keyAttributes: std::ptr::null(),
        };
        let mut data = std::ptr::null();
        // SAFETY: valid identity ref and params that outlive the call; on
        // success `data` is a +1 CFData we take ownership of.
        let status = unsafe { SecItemExport(self.handle.as_CFTypeRef(), kSecFormatPKCS12, 0, &params, &mut data) };
        if status != 0 || data.is_null() {
            bail!(
                "exporting {} from the keychain failed (OSStatus {status}); was the export dialog denied?",
                self.info.short_sha1()
            );
        }
        Ok(unsafe { CFData::wrap_under_create_rule(data) }.bytes().to_vec())
    }
}

