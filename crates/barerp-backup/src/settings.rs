//! `backup-settings.json`: mesmo formato e mesmas validações do servidor Node.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::{msg, BackupError};

pub(crate) const SETTINGS_FILE: &str = "backup-settings.json";
const DEFAULT_KEEP: u32 = 30;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSettings {
    pub external_folder: String,
    /// Liga a cópia para a pasta externa (o backup local roda sempre).
    pub enabled: bool,
    /// Dias de histórico (ver [`crate::plan_prune`]).
    pub keep: u32,
}

impl Default for BackupSettings {
    fn default() -> Self {
        Self {
            external_folder: String::new(),
            enabled: false,
            keep: DEFAULT_KEEP,
        }
    }
}

/// Alteração vinda da tela: campo ausente mantém o valor atual.
/// `keep` aceita número ou texto, como o `Number(...)` do Node.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupSettingsInput {
    #[serde(default)]
    pub external_folder: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    pub keep: Option<serde_json::Value>,
}

/// Distingue `"keep": null` (vira o padrão, como no Node) de campo ausente.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<serde_json::Value>, D::Error> {
    serde_json::Value::deserialize(d).map(Some)
}

/// `Math.floor(Number(v))` entre 1 e 365; inválido vira 30.
pub(crate) fn normalize_keep(v: &serde_json::Value) -> u32 {
    use serde_json::Value;
    let n = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) if s.trim().is_empty() => 0.0,
        Value::String(s) => s.trim().parse::<f64>().unwrap_or(f64::NAN),
        Value::Bool(b) => *b as u8 as f64,
        Value::Null => 0.0,
        _ => f64::NAN,
    }
    .floor();
    if n.is_finite() && n >= 1.0 {
        n.min(365.0) as u32
    } else {
        DEFAULT_KEEP
    }
}

/// Lê a configuração; arquivo ausente ou inválido = padrão, campo com tipo errado = padrão do campo.
pub(crate) fn load(base_dir: &Path) -> BackupSettings {
    let Ok(text) = std::fs::read_to_string(base_dir.join(SETTINGS_FILE)) else {
        return BackupSettings::default();
    };
    let Ok(serde_json::Value::Object(raw)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return BackupSettings::default();
    };
    let d = BackupSettings::default();
    BackupSettings {
        external_folder: raw
            .get("externalFolder")
            .and_then(|v| v.as_str())
            .map_or(d.external_folder, str::to_string),
        enabled: raw
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(d.enabled),
        keep: raw.get("keep").map_or(d.keep, normalize_keep),
    }
}

/// Valida e grava. Mesmas regras e mensagens do servidor Node.
pub(crate) fn save(
    base_dir: &Path,
    input: &BackupSettingsInput,
) -> Result<BackupSettings, BackupError> {
    let current = load(base_dir);
    let next = BackupSettings {
        external_folder: input
            .external_folder
            .as_deref()
            .map_or(current.external_folder, |s| s.trim().to_string()),
        enabled: input.enabled.unwrap_or(current.enabled),
        keep: input.keep.as_ref().map_or(current.keep, normalize_keep),
    };

    if next.enabled && next.external_folder.is_empty() {
        return Err(msg(
            "Escolha a pasta de backup externo antes de ativar a cópia.",
        ));
    }
    if !next.external_folder.is_empty() {
        let folder = Path::new(&next.external_folder);
        if !folder.is_absolute() {
            return Err(msg(
                "Informe o caminho completo da pasta (ex.: C:\\Users\\Bar\\Google Drive\\Backups).",
            ));
        }
        if is_inside(&lexical(folder), &lexical(base_dir)) {
            return Err(msg(
                "A pasta externa não pode ficar dentro da pasta do banco de dados. Use o Google Drive, OneDrive ou um pen drive.",
            ));
        }
        // Só exige a pasta acessível quando a cópia está ligada (o pen drive pode estar fora na hora de desligar)
        if next.enabled {
            assert_writable_folder(folder)?;
        }
    }

    let json = serde_json::to_string_pretty(&next).expect("serializa sempre");
    std::fs::write(base_dir.join(SETTINGS_FILE), json)?;
    Ok(next)
}

/// Resolve `.` e `..` sem tocar no disco (como `path.resolve`).
fn lexical(p: &Path) -> PathBuf {
    let p = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `child` é `parent` ou fica dentro dele (no Windows sem diferenciar maiúsculas).
fn is_inside(child: &Path, parent: &Path) -> bool {
    if cfg!(windows) {
        let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
        lower(child).starts_with(lower(parent))
    } else {
        child.starts_with(parent)
    }
}

/// Pasta existe, é pasta e aceita gravação. Erro com mensagem para o operador.
pub(crate) fn assert_writable_folder(folder: &Path) -> Result<(), BackupError> {
    let shown = folder.display();
    let meta = std::fs::metadata(folder).map_err(|_| {
        msg(format!(
            "A pasta de backup externo não foi encontrada: {shown}. Verifique se o pen drive está conectado ou se a pasta do Google Drive/OneDrive existe."
        ))
    })?;
    if !meta.is_dir() {
        return Err(msg(format!("O caminho escolhido não é uma pasta: {shown}")));
    }
    let millis = chrono::Utc::now().timestamp_millis();
    let probe = folder.join(format!(
        ".bar-erp-teste-{}-{millis}.tmp",
        std::process::id()
    ));
    std::fs::write(&probe, "ok")
        .and_then(|_| std::fs::remove_file(&probe))
        .map_err(|_| {
            msg(format!(
                "Sem permissão para gravar na pasta de backup externo: {shown}"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keep_como_number_do_node() {
        assert_eq!(normalize_keep(&json!(7)), 7);
        assert_eq!(normalize_keep(&json!(7.9)), 7);
        assert_eq!(normalize_keep(&json!("15")), 15);
        assert_eq!(normalize_keep(&json!(1000)), 365);
        assert_eq!(normalize_keep(&json!(0)), 30);
        assert_eq!(normalize_keep(&json!("abc")), 30);
        assert_eq!(normalize_keep(&json!(null)), 30);
        assert_eq!(normalize_keep(&json!(true)), 1);
    }

    #[test]
    fn input_distingue_keep_null_de_ausente() {
        let a: BackupSettingsInput = serde_json::from_str(r#"{"enabled": false}"#).unwrap();
        assert_eq!(a.keep, None);
        let b: BackupSettingsInput = serde_json::from_str(r#"{"keep": null}"#).unwrap();
        assert_eq!(b.keep, Some(json!(null)));
    }

    #[test]
    fn pasta_dentro_do_banco() {
        let base = Path::new("/dados/bar");
        assert!(is_inside(&lexical(Path::new("/dados/bar/backups")), base));
        assert!(is_inside(&lexical(Path::new("/dados/bar")), base));
        assert!(!is_inside(&lexical(Path::new("/dados/bar/../drive")), base));
        assert!(!is_inside(&lexical(Path::new("/dados/barra")), base));
    }
}
