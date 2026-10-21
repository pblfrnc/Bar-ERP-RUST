//! Nomes dos arquivos de backup, listagem e retenção.

use std::path::Path;

use chrono::{DateTime, Local, Utc};
use serde::Serialize;

pub(crate) const LOCAL_PREFIX: &str = "backup_";
pub(crate) const EXTERNAL_PREFIX: &str = "bar-erp-backup_";
const KEEP_ALL_DAYS: usize = 2;

/// Arquivo de backup e quando foi gravado (`at` no formato do `toISOString()`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BackupFileInfo {
    pub file: String,
    pub at: String,
}

/// `2026-10-18_2130`
pub(crate) fn stamp(d: DateTime<Local>) -> String {
    d.format("%Y-%m-%d_%H%M").to_string()
}

pub(crate) fn day(d: DateTime<Local>) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub(crate) fn iso(d: DateTime<Utc>) -> String {
    d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 10
        && b.iter().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                *c == b'-'
            } else {
                c.is_ascii_digit()
            }
        })
}

fn is_hhmm(s: &str) -> bool {
    s.len() == 4 && s.bytes().all(|c| c.is_ascii_digit())
}

/// `backup_2026-10-17.db` (cópia diária antiga do Electron) ou `backup_2026-10-17_2130.db`.
pub(crate) fn is_local_backup(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(LOCAL_PREFIX)
        .and_then(|r| r.strip_suffix(".db"))
    else {
        return false;
    };
    match rest.split_once('_') {
        None => is_date(rest),
        Some((d, t)) => is_date(d) && is_hhmm(t),
    }
}

/// `bar-erp-backup_2026-10-17_2130.db`
pub(crate) fn is_external_backup(name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(EXTERNAL_PREFIX)
        .and_then(|r| r.strip_suffix(".db"))
    else {
        return false;
    };
    rest.split_once('_')
        .is_some_and(|(d, t)| is_date(d) && is_hhmm(t))
}

/// Backups da pasta, mais novo primeiro (a ordem do nome é a ordem da data). Pasta ausente = vazio.
pub(crate) fn list(dir: &Path, matches: fn(&str) -> bool) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|f| matches(f))
        .collect();
    files.sort();
    files.reverse();
    files
}

/// Quais arquivos apagar: mantém todos os backups dos 2 dias mais recentes e o mais novo de cada dia
/// anterior, até `keep_days` dias. Conta dias que têm backup (não dias do calendário): um PC desligado
/// por semanas não perde o histórico. `files` pode vir em qualquer ordem.
pub fn plan_prune(files: &[String], keep_days: usize) -> Vec<String> {
    let mut sorted: Vec<&String> = files.iter().collect();
    sorted.sort();
    sorted.reverse();
    let mut days: Vec<(&str, Vec<&String>)> = Vec::new();
    for f in sorted {
        let Some(d) = date_in(f) else { continue };
        match days.last_mut() {
            Some((last, list)) if *last == d => list.push(f),
            _ => days.push((d, vec![f])),
        }
    }
    let mut delete = Vec::new();
    for (index, (_, list)) in days.iter().enumerate() {
        if index >= keep_days {
            delete.extend(list.iter().map(|f| f.to_string()));
        } else if index >= KEEP_ALL_DAYS {
            delete.extend(list.iter().skip(1).map(|f| f.to_string()));
        }
    }
    delete
}

/// Primeira data `AAAA-MM-DD` do nome.
fn date_in(name: &str) -> Option<&str> {
    (0..name.len().saturating_sub(9)).find_map(|i| name.get(i..i + 10).filter(|s| is_date(s)))
}

pub(crate) fn prune(dir: &Path, matches: fn(&str) -> bool, keep_days: usize) {
    for old in plan_prune(&list(dir, matches), keep_days) {
        match std::fs::remove_file(dir.join(&old)) {
            Ok(()) => tracing::info!("[Backup] Removido backup antigo: {old}"),
            Err(e) => tracing::warn!("[Backup] Não foi possível remover {old}: {e}"),
        }
    }
}

pub(crate) fn file_info(dir: &Path, file: Option<&str>) -> Option<BackupFileInfo> {
    let file = file?;
    let modified = std::fs::metadata(dir.join(file))
        .and_then(|m| m.modified())
        .ok()?;
    Some(BackupFileInfo {
        file: file.to_string(),
        at: iso(modified.into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reconhece_nomes() {
        assert!(is_local_backup("backup_2026-10-17.db"));
        assert!(is_local_backup("backup_2026-10-17_2130.db"));
        assert!(!is_local_backup("backup_2026-10-17_2130.db.tmp"));
        assert!(!is_local_backup("backup_2026-1-17.db"));
        assert!(!is_local_backup("antes-da-restauracao_2026-10-17_2130.db"));
        assert!(is_external_backup("bar-erp-backup_2026-10-17_2130.db"));
        assert!(!is_external_backup("bar-erp-backup_2026-10-17.db"));
    }

    #[test]
    fn retencao_dois_dias_completos_e_um_por_dia() {
        let files = names(&[
            "backup_2026-10-18_0900.db",
            "backup_2026-10-18_1500.db",
            "backup_2026-10-17_0900.db",
            "backup_2026-10-17_2100.db",
            "backup_2026-10-15_0800.db",
            "backup_2026-10-15_2000.db",
            "backup_2026-10-15.db",
            "backup_2026-10-10_1000.db",
            "backup_2026-10-01_1000.db",
        ]);
        let mut del = plan_prune(&files, 4);
        del.sort();
        // 18 e 17 inteiros; do dia 15 fica só o mais novo (2000); dia 10 é o 4º dia; dia 01 passa do limite
        assert_eq!(
            del,
            names(&[
                "backup_2026-10-01_1000.db",
                "backup_2026-10-15.db",
                "backup_2026-10-15_0800.db"
            ])
        );
    }

    #[test]
    fn retencao_com_um_dia_so() {
        let files = names(&[
            "backup_2026-10-18_0900.db",
            "backup_2026-10-18_1500.db",
            "backup_2026-10-17_0900.db",
        ]);
        assert_eq!(plan_prune(&files, 1), names(&["backup_2026-10-17_0900.db"]));
    }
}
