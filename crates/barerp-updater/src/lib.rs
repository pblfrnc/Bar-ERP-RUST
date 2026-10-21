//! Atualizador do BarERP, tradução de `electron/updater.cjs`. Usa a mesma release do GitHub
//! (`pblfrnc/bar-erp`, tag `latest`): `version.json` e `BarERP-Instalador-Windows.exe`.
//!
//! - [`check`]: três estratégias, como no Node: `version.json` da release (sem limite de API), feed
//!   `releases.atom` + `HEAD` do instalador, e por último a API REST do GitHub.
//! - [`download`]: baixa o instalador para a pasta temporária com progresso.
//! - [`install`]: abre o instalador desanexado; quem chama fecha o app logo depois (arquivos travados).
//!
//! A versão local vem do `Cargo.toml`; o commit e a hora do build vêm das variáveis `BARERP_COMMIT` e
//! `BARERP_BUILD_TIME` no momento da compilação (o CI define; sem elas vale "local" e 01/01/2026, como o Node).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::Deserialize;

pub const INSTALLER_NAME: &str = "BarERP-Instalador-Windows.exe";

/// Endereços do GitHub (trocáveis nos testes).
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub version_json: String,
    pub installer: String,
    pub releases_atom: String,
    pub api_latest: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        let base = "https://github.com/pblfrnc/bar-erp";
        Self {
            version_json: format!("{base}/releases/download/latest/version.json"),
            installer: format!("{base}/releases/download/latest/{INSTALLER_NAME}"),
            releases_atom: format!("{base}/releases.atom"),
            api_latest: "https://api.github.com/repos/pblfrnc/bar-erp/releases/latest".into(),
        }
    }
}

/// Build instalado (o `build-info.json` do Electron).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalBuild {
    pub version: String,
    pub commit: String,
    pub build_time: DateTime<Utc>,
}

impl LocalBuild {
    pub fn current() -> Self {
        let build_time = option_env!("BARERP_BUILD_TIME")
            .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(|| DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").expect("data fixa").with_timezone(&Utc));
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            commit: option_env!("BARERP_COMMIT").unwrap_or("local").to_string(),
            build_time,
        }
    }
}

/// Resultado da checagem (os campos do `UpdateInfo` do original).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    /// "v2.1.0" ou o título da release
    pub latest_version: String,
    pub release_name: String,
    pub release_notes: String,
    pub asset_date: Option<DateTime<Utc>>,
    pub download_url: String,
    pub file_size: u64,
    /// Mensagem quando não há atualização ("Você já está na versão mais recente...")
    pub message: Option<String>,
}

/// Comparador numérico de versões (`1.6.9` vs `1.10.0`), como o `compareVersions` do Node.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |v: &str| -> Vec<u64> {
        v.trim().trim_start_matches(['v', 'V']).split('.').map(|n| n.trim().parse::<u64>().unwrap_or(0)).collect()
    };
    let (pa, pb) = (parse(a), parse(b));
    for i in 0..pa.len().max(pb.len()) {
        let (x, y) = (pa.get(i).copied().unwrap_or(0), pb.get(i).copied().unwrap_or(0));
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

/// Primeira versão `x.y.z` dentro de um texto ("BarERP Pro v2.1.0").
fn version_in(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() || (start > 0 && bytes[start - 1].is_ascii_digit()) {
            continue;
        }
        let rest = &text[start..];
        let candidate: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let parts: Vec<&str> = candidate.split('.').collect();
        if parts.len() >= 3 && parts[..3].iter().all(|p| !p.is_empty()) {
            return Some(parts[..3].join("."));
        }
    }
    None
}

fn client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent("BarERP-AutoUpdater")
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("cliente HTTP")
}

fn no_cache(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header("Cache-Control", "no-cache, no-store, must-revalidate").header("Pragma", "no-cache")
}

#[derive(Deserialize)]
struct VersionJson {
    version: Option<String>,
    commit: Option<String>,
    #[serde(rename = "buildTime")]
    build_time: Option<String>,
}

struct Head {
    size: Option<u64>,
    modified: Option<DateTime<Utc>>,
}

async fn head_installer(http: &reqwest::Client, url: &str) -> Option<Head> {
    let res = no_cache(http.head(url)).send().await.ok()?;
    if !res.status().is_success() {
        return None;
    }
    let header = |name: &str| res.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
    Some(Head {
        size: header("content-length").and_then(|v| v.parse().ok()),
        modified: header("last-modified")
            .and_then(|v| DateTime::parse_from_rfc2822(&v).ok())
            .map(|d| d.with_timezone(&Utc)),
    })
}

fn parse_date(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|d| d.with_timezone(&Utc))
}

/// Confere se há versão nova (as três estratégias do Node, na mesma ordem).
pub async fn check(endpoints: &Endpoints, local: &LocalBuild) -> UpdateInfo {
    let http = client(Duration::from_secs(25));

    // 1. version.json da release
    let url = format!("{}?_t={}", endpoints.version_json, Utc::now().timestamp_millis());
    if let Ok(res) = no_cache(http.get(&url)).send().await {
        if res.status().is_success() {
            if let Ok(remote) = res.json::<VersionJson>().await {
                let remote_version = remote.version.clone().unwrap_or_else(|| local.version.clone());
                let diff = compare_versions(&remote_version, &local.version);
                if diff == std::cmp::Ordering::Less {
                    return UpdateInfo {
                        current_version: local.version.clone(),
                        latest_version: remote_version,
                        message: Some("Você já está utilizando uma compilação mais recente.".into()),
                        ..Default::default()
                    };
                }
                let same_commit = local.commit != "local" && remote.commit.as_deref() == Some(local.commit.as_str());
                if diff == std::cmp::Ordering::Equal && same_commit {
                    return UpdateInfo {
                        current_version: local.version.clone(),
                        latest_version: remote_version,
                        message: Some(format!("Você já está na versão mais recente (v{}).", local.version)),
                        ..Default::default()
                    };
                }
                let head = head_installer(&http, &endpoints.installer).await;
                let short_commit = remote.commit.as_deref().map(|c| c.chars().take(7).collect::<String>());
                return UpdateInfo {
                    has_update: true,
                    current_version: local.version.clone(),
                    latest_version: remote.version.as_ref().map(|v| format!("v{v}")).unwrap_or_else(|| "Nova Versão".into()),
                    release_name: format!(
                        "BarERP Pro v{} ({})",
                        remote.version.as_deref().unwrap_or(&local.version),
                        short_commit.as_deref().unwrap_or("Atualização")
                    ),
                    release_notes: "Nova compilação do BarERP com melhorias de sistema e atualizações.".into(),
                    asset_date: head
                        .as_ref()
                        .and_then(|h| h.modified)
                        .or_else(|| remote.build_time.as_deref().and_then(parse_date))
                        .or(Some(Utc::now())),
                    download_url: endpoints.installer.clone(),
                    file_size: head.and_then(|h| h.size).unwrap_or(110_569_297),
                    message: None,
                };
            }
        }
    }

    // 2. Feed público + HEAD do instalador
    if let Some(head) = head_installer(&http, &endpoints.installer).await {
        if let Some(asset_date) = head.modified {
            let mut title = "BarERP - Nova Versão".to_string();
            let mut notes = "Melhorias de desempenho, correções e novos recursos.".to_string();
            if let Ok(res) = http.get(&endpoints.releases_atom).send().await {
                if res.status().is_success() {
                    if let Ok(xml) = res.text().await {
                        if let Some(entry) = xml.split("<entry>").nth(1) {
                            if let Some(t) = between(entry, "<title>", "</title>") {
                                title = t.to_string();
                            }
                            if let Some(c) = between(entry, "<content type=\"html\">", "</content>") {
                                notes = strip_html(&c.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"));
                            }
                        }
                    }
                }
            }
            let remote_version = version_in(&title);
            let diff = remote_version.as_deref().map(|v| compare_versions(v, &local.version)).unwrap_or(std::cmp::Ordering::Equal);
            let newer = diff == std::cmp::Ordering::Greater
                || (diff != std::cmp::Ordering::Less && (asset_date - local.build_time).num_milliseconds() > 60_000);
            return UpdateInfo {
                has_update: newer,
                current_version: local.version.clone(),
                latest_version: remote_version.map(|v| format!("v{v}")).unwrap_or_else(|| title.clone()),
                release_name: title,
                release_notes: notes,
                asset_date: Some(asset_date),
                download_url: endpoints.installer.clone(),
                file_size: head.size.unwrap_or(0),
                message: None,
            };
        }
    }

    // 3. API REST do GitHub
    if let Ok(res) = http.get(&endpoints.api_latest).send().await {
        if res.status().is_success() {
            if let Ok(release) = res.json::<serde_json::Value>().await {
                let asset = release["assets"].as_array().and_then(|list| {
                    list.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with("-Instalador-Windows.exe") || n.ends_with(".exe")))
                });
                if let Some(asset) = asset {
                    let tag = release["tag_name"].as_str().or(release["name"].as_str()).unwrap_or("");
                    let remote_version = version_in(tag);
                    let diff = remote_version.as_deref().map(|v| compare_versions(v, &local.version)).unwrap_or(std::cmp::Ordering::Equal);
                    let updated = asset["updated_at"].as_str().or(release["published_at"].as_str()).and_then(parse_date);
                    let newer = diff == std::cmp::Ordering::Greater
                        || (diff != std::cmp::Ordering::Less
                            && updated.is_some_and(|u| (u - local.build_time).num_milliseconds() > 60_000));
                    return UpdateInfo {
                        has_update: newer,
                        current_version: local.version.clone(),
                        latest_version: remote_version.map(|v| format!("v{v}")).unwrap_or_else(|| tag.to_string()),
                        release_name: release["name"].as_str().unwrap_or("Atualização do BarERP").to_string(),
                        release_notes: release["body"].as_str().unwrap_or("Melhorias gerais e correções de desempenho.").to_string(),
                        asset_date: updated,
                        download_url: asset["browser_download_url"].as_str().unwrap_or(&endpoints.installer).to_string(),
                        file_size: asset["size"].as_u64().unwrap_or(0),
                        message: None,
                    };
                }
            }
        }
    }

    UpdateInfo {
        current_version: local.version.clone(),
        message: Some("Não foi possível verificar atualizações no momento.".into()),
        ..Default::default()
    }
}

fn between<'a>(text: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = text.find(start)? + start.len();
    let j = text[i..].find(end)? + i;
    Some(&text[i..j])
}

fn strip_html(text: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// Progresso do download (a cada ~200 ms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub percent: u32,
    pub received: u64,
    pub total: u64,
    /// MB/s no último intervalo
    pub speed_mbps: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error("Falha no download da atualização (Status {0})")]
    Status(u16),
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Instalador da atualização não encontrado. Faça o download novamente.")]
    Missing,
}

/// Onde o instalador baixado fica.
pub fn installer_path() -> PathBuf {
    std::env::temp_dir().join("BarERP-Instalador-Update.exe")
}

/// Baixa o instalador para `target` com o progresso em `on_progress`.
pub async fn download(url: &str, target: &Path, mut on_progress: impl FnMut(Progress)) -> Result<PathBuf, UpdateError> {
    use tokio::io::AsyncWriteExt;
    let _ = tokio::fs::remove_file(target).await;
    let http = client(Duration::from_secs(60 * 30));
    let mut res = http.get(url).send().await?;
    if res.status().as_u16() != 200 {
        return Err(UpdateError::Status(res.status().as_u16()));
    }
    let total = res.content_length().unwrap_or(0);
    let partial = target.with_extension("part");
    let mut file = tokio::fs::File::create(&partial).await?;
    let (mut received, mut last_bytes, mut last_time) = (0u64, 0u64, Instant::now());
    let result: Result<(), UpdateError> = async {
        while let Some(chunk) = res.chunk().await? {
            file.write_all(&chunk).await?;
            received += chunk.len() as u64;
            let elapsed = last_time.elapsed();
            if elapsed >= Duration::from_millis(200) || received == total {
                let speed = (received - last_bytes) as f64 / elapsed.as_secs_f64().max(0.001) / (1024.0 * 1024.0);
                let percent = if total > 0 { ((received as f64 / total as f64) * 100.0).round() as u32 } else { 0 };
                on_progress(Progress { percent, received, total, speed_mbps: speed });
                last_time = Instant::now();
                last_bytes = received;
            }
        }
        file.flush().await?;
        Ok(())
    }
    .await;
    drop(file);
    if let Err(e) = result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(e);
    }
    tokio::fs::rename(&partial, target).await?;
    Ok(target.to_path_buf())
}

/// Abre o instalador sem ficar preso ao app (o instalador NSIS atualiza os arquivos e reabre o BarERP).
/// Quem chama deve fechar o app em seguida.
pub fn install(installer: &Path) -> Result<(), UpdateError> {
    if !installer.exists() {
        return Err(UpdateError::Missing);
    }
    let mut cmd = std::process::Command::new(installer);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: o instalador continua depois que o app fecha
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    cmd.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn versions_like_node() {
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("v2.0.0", "2.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.9.0", "2.0.0"), Ordering::Less);
        assert_eq!(version_in("BarERP Pro v2.1.3 (abc)").as_deref(), Some("2.1.3"));
        assert_eq!(version_in("latest"), None);
    }

    #[test]
    fn html_notes() {
        assert_eq!(strip_html("<p>Novo <b>caixa</b></p>"), "Novo caixa");
    }
}
