//! 암호 걸기·풀기. lopdf 0.46의 표준 보안 처리기(Standard security handler)를 쓴다:
//! AES-128 = V4/R4(AESV2 crypt filter), AES-256 = V5/R6(AESV3). 파일 키는 운영체제 난수(getrandom).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
use lopdf::{dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions};
use serde::{Deserialize, Serialize};

use super::{open, EditOut};
use crate::error::{io_err, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Cipher {
    Aes128,
    #[default]
    Aes256,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtectOpts {
    /// 열 때 묻는 암호. 비우면 누구나 열 수 있고 아래 제한만 걸린다.
    #[serde(default)]
    pub open_password: String,
    /// 제한을 바꿀 때 쓰는 암호(소유자 암호). 비우면 무작위로 만든다(제한을 풀 수 없게).
    #[serde(default)]
    pub owner_password: String,
    #[serde(default = "yes")]
    pub allow_print: bool,
    #[serde(default = "yes")]
    pub allow_copy: bool,
    #[serde(default = "yes")]
    pub allow_edit: bool,
    #[serde(default)]
    pub cipher: Cipher,
}

fn random_bytes<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0u8; N];
    getrandom::fill(&mut b).map_err(|e| Error::Invalid(format!("난수를 만들지 못했습니다: {e}")))?;
    Ok(b)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn permissions(opts: &ProtectOpts) -> Permissions {
    let mut p = Permissions::all();
    if !opts.allow_print {
        p.remove(Permissions::PRINTABLE | Permissions::PRINTABLE_IN_HIGH_QUALITY);
    }
    if !opts.allow_copy {
        p.remove(Permissions::COPYABLE);
    }
    if !opts.allow_edit {
        p.remove(Permissions::MODIFIABLE | Permissions::ANNOTABLE | Permissions::FILLABLE | Permissions::ASSEMBLABLE);
    }
    p
}

/// 암호를 건 새 PDF. `password`는 입력 PDF가 이미 암호 PDF일 때 여는 암호.
pub fn protect(input: &Path, password: Option<&str>, opts: &ProtectOpts, output: &Path) -> Result<EditOut> {
    let all_allowed = opts.allow_print && opts.allow_copy && opts.allow_edit;
    if opts.open_password.is_empty() && all_allowed {
        return Err(Error::Invalid("열기 암호를 넣거나 인쇄·복사·편집 제한을 하나 이상 고르세요".into()));
    }
    if opts.open_password.chars().count() > 127 || opts.owner_password.chars().count() > 127 {
        return Err(Error::Invalid("암호는 127자까지 됩니다".into()));
    }
    if opts.cipher == Cipher::Aes128 && !(opts.open_password.is_ascii() && opts.owner_password.is_ascii()) {
        return Err(Error::Invalid("AES-128 암호에는 영문·숫자·기호만 쓸 수 있습니다. 한글 암호는 AES-256을 고르세요".into()));
    }
    if output.exists() {
        return Err(Error::AlreadyExists(output.to_path_buf()));
    }
    let mut doc = open(input, password)?;
    let owner = if opts.owner_password.is_empty() { hex(&random_bytes::<16>()?) } else { opts.owner_password.clone() };
    if !opts.open_password.is_empty() && owner == opts.open_password && !all_allowed {
        return Err(Error::Invalid("제한을 걸 때는 소유자 암호를 열기 암호와 다르게 정하세요".into()));
    }
    // 파일 ID(암호 키 계산에 쓰임)
    if doc.trailer.get(b"ID").is_err() {
        let id = random_bytes::<16>()?;
        doc.trailer.set("ID", Object::Array(vec![Object::string_literal(id.to_vec()), Object::string_literal(id.to_vec())]));
    }
    doc.prune_objects();
    doc.compress();
    let perms = permissions(opts);
    let filters = |f: Arc<dyn CryptFilter>| BTreeMap::from([(b"StdCF".to_vec(), f)]);
    let key = random_bytes::<32>()?;
    let version = match opts.cipher {
        Cipher::Aes128 => EncryptionVersion::V4 {
            document: &doc,
            encrypt_metadata: true,
            crypt_filters: filters(Arc::new(Aes128CryptFilter)),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: &owner,
            user_password: &opts.open_password,
            permissions: perms,
        },
        Cipher::Aes256 => EncryptionVersion::V5 {
            encrypt_metadata: true,
            crypt_filters: filters(Arc::new(Aes256CryptFilter)),
            file_encryption_key: &key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: &owner,
            user_password: &opts.open_password,
            permissions: perms,
        },
    };
    let state = EncryptionState::try_from(version).map_err(|e| Error::Invalid(format!("암호를 걸지 못했습니다: {e}")))?;
    if opts.cipher == Cipher::Aes256 {
        // AES-256(R6)은 PDF 2.0 / Adobe 확장 8
        doc.version = "1.7".into();
        if let Ok(cat) = doc.catalog_mut() {
            cat.set("Extensions", dictionary! { "ADBE" => dictionary! { "BaseVersion" => Object::Name(b"1.7".to_vec()), "ExtensionLevel" => 8 } });
        }
    } else if doc.version.as_str() < "1.6" {
        doc.version = "1.6".into();
    }
    doc.encrypt(&state).map_err(|e| Error::Invalid(format!("암호를 걸지 못했습니다: {e}")))?;
    let mut bytes = Vec::new();
    doc.save_to(&mut bytes).map_err(io_err(output))?;
    crate::paths::write_new(output, &bytes)?;
    let pages = doc.get_pages().len() as u32;
    let mut parts = Vec::new();
    if !opts.open_password.is_empty() {
        parts.push("열기 암호".to_string());
    }
    let mut lim = Vec::new();
    if !opts.allow_print {
        lim.push("인쇄");
    }
    if !opts.allow_copy {
        lim.push("복사");
    }
    if !opts.allow_edit {
        lim.push("편집");
    }
    if !lim.is_empty() {
        parts.push(format!("{} 제한", lim.join("·")));
    }
    let cipher = match opts.cipher {
        Cipher::Aes128 => "AES-128",
        Cipher::Aes256 => "AES-256",
    };
    Ok(EditOut { output: output.to_path_buf(), pages, note: format!("{}을 걸었습니다({cipher})", parts.join(", ")) })
}

#[derive(Debug, Clone, Serialize)]
pub struct Protection {
    /// 열 때 암호가 필요한지
    pub needs_password: bool,
    /// 암호가 걸려 있는지(열기 암호 또는 제한만)
    pub encrypted: bool,
    pub allow_print: Option<bool>,
    pub allow_copy: Option<bool>,
    pub allow_edit: Option<bool>,
    /// "AES-256", "AES-128", "RC4" …
    pub method: Option<String>,
}

fn method_of(state: &EncryptionState) -> String {
    match (state.version(), state.revision()) {
        (5, _) => "AES-256".into(),
        (4, _) => {
            let m = state.crypt_filters().values().next().map(|f| f.method().to_vec()).unwrap_or_default();
            match m.as_slice() {
                b"AESV2" => "AES-128".into(),
                b"V2" => "RC4-128".into(),
                _ => "AES/RC4".into(),
            }
        }
        (v, _) => format!("RC4 (V{v})"),
    }
}

/// 암호 상태 보기(열기 암호가 있으면 `password`가 있어야 권한까지 보임)
pub fn protection(input: &Path, password: Option<&str>) -> Result<Protection> {
    let raw = Document::load(input).map_err(super::pdf_err(input))?;
    if raw.is_encrypted() {
        // 열기 암호가 있음
        let Some(pw) = password.filter(|s| !s.is_empty()) else {
            return Ok(Protection { needs_password: true, encrypted: true, allow_print: None, allow_copy: None, allow_edit: None, method: None });
        };
        let doc = open(input, Some(pw))?;
        return Ok(describe(&doc, true));
    }
    Ok(describe(&raw, false))
}

fn describe(doc: &Document, needs_password: bool) -> Protection {
    match doc.encryption_state.as_ref() {
        Some(s) => {
            let p = s.permissions();
            Protection {
                needs_password,
                encrypted: true,
                allow_print: Some(p.contains(Permissions::PRINTABLE)),
                allow_copy: Some(p.contains(Permissions::COPYABLE)),
                allow_edit: Some(p.contains(Permissions::MODIFIABLE)),
                method: Some(method_of(s)),
            }
        }
        None => Protection { needs_password, encrypted: false, allow_print: None, allow_copy: None, allow_edit: None, method: None },
    }
}

/// 아는 암호로 보호를 푼다. 열기 암호가 있는 PDF는 그 암호(또는 소유자 암호),
/// 제한만 걸린 PDF는 **소유자 암호**가 있어야 한다.
pub fn unprotect(input: &Path, password: &str, output: &Path) -> Result<EditOut> {
    if password.is_empty() {
        return Err(Error::PasswordRequired(input.to_path_buf()));
    }
    let raw = Document::load(input).map_err(super::pdf_err(input))?;
    let doc = if raw.is_encrypted() {
        open(input, Some(password))?
    } else {
        let Some(state) = raw.encryption_state.as_ref() else {
            return Err(Error::Invalid("암호가 걸려 있지 않은 PDF입니다".into()));
        };
        // 제한만 걸린 PDF: 소유자 암호 확인(읽을 때 지워진 /Encrypt 사전을 다시 만들어 확인)
        let enc = state.encode().map_err(|e| Error::Invalid(e.to_string()))?;
        let mut probe = Document::with_version("1.7");
        let id = probe.add_object(enc);
        probe.trailer.set("Encrypt", id);
        if let Ok(fid) = raw.trailer.get(b"ID") {
            probe.trailer.set("ID", fid.clone());
        }
        if probe.authenticate_owner_password(password).is_err() {
            return Err(Error::WrongPassword(input.to_path_buf()));
        }
        raw
    };
    let pages = doc.get_pages().len() as u32;
    super::save(doc, output)?;
    Ok(EditOut { output: output.to_path_buf(), pages, note: "암호를 풀었습니다".into() })
}
