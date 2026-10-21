//! Persistência fiscal: `FiscalSettings`, `NotaEmitida`, `NotaRecebida` e o cofre de XML em disco.
//!
//! Tradução das partes de banco de `server/src/routes/fiscal.ts` e `server/src/services/nfeService.ts`
//! (`persistNfeRecord`, `/recent-notes`, `/notas-pendentes`, `/month-summary`...). A conversa com a Focus NFe
//! fica no `barerp-fiscal`; este módulo não depende dele.

use std::io;
use std::path::{Path, PathBuf};

use barerp_core::{ids, vault};
use chrono::{DateTime, Datelike, Local, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqliteConnection};

use crate::{Db, DbError, DbResult, DbTime};

const CONFIG_ID: &str = "default";

// ============================================================
// FiscalSettings
// ============================================================

/// Linha `FiscalSettings` com `apiToken` e `cscSecret` já abertos.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct FiscalSettings {
    #[serde(default)]
    pub id: String,
    #[sqlx(rename = "apiToken")]
    pub api_token: Option<String>,
    pub cnpj: Option<String>,
    pub ie: Option<String>,
    /// "3" = regime normal; qualquer outro valor é Simples Nacional.
    pub crt: Option<String>,
    #[sqlx(rename = "cscId")]
    pub csc_id: Option<String>,
    #[sqlx(rename = "cscSecret")]
    pub csc_secret: Option<String>,
    #[sqlx(rename = "addressInfo")]
    pub address_info: Option<String>,
    /// "producao" ou "homologacao".
    pub environment: Option<String>,
    #[sqlx(rename = "serieNfce")]
    pub serie_nfce: Option<String>,
    #[sqlx(rename = "proximoNumeroNfce")]
    pub proximo_numero_nfce: Option<i64>,
    #[sqlx(rename = "serieNfe")]
    pub serie_nfe: Option<String>,
    #[sqlx(rename = "proximoNumeroNfe")]
    pub proximo_numero_nfe: Option<i64>,
    pub cep: Option<String>,
    pub logradouro: Option<String>,
    pub numero: Option<String>,
    pub bairro: Option<String>,
    pub municipio: Option<String>,
    pub uf: Option<String>,
    #[sqlx(rename = "razaoSocial")]
    pub razao_social: Option<String>,
    #[sqlx(rename = "nomeFantasia")]
    pub nome_fantasia: Option<String>,
    #[sqlx(rename = "updatedAt")]
    #[serde(default)]
    pub updated_at: Option<DbTime>,
}

/// Dados do emitente para montar a nota (mesmos campos de `barerp_fiscal::Emitente`; vazio = não configurado).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DadosEmitente {
    pub cnpj: String,
    pub crt: String,
    pub serie_nfce: String,
    pub serie_nfe: String,
    pub cep: String,
    pub logradouro: String,
    pub numero: String,
    pub bairro: String,
    pub municipio: String,
    pub uf: String,
}

impl FiscalSettings {
    pub fn emitente(&self) -> DadosEmitente {
        let s = |v: &Option<String>| v.clone().unwrap_or_default();
        DadosEmitente {
            cnpj: s(&self.cnpj),
            crt: s(&self.crt),
            serie_nfce: s(&self.serie_nfce),
            serie_nfe: s(&self.serie_nfe),
            cep: s(&self.cep),
            logradouro: s(&self.logradouro),
            numero: s(&self.numero),
            bairro: s(&self.bairro),
            municipio: s(&self.municipio),
            uf: s(&self.uf),
        }
    }

    /// Token da Focus pronto para uso (sem espaços). `None` se não configurado ou se não abriu.
    pub fn token(&self) -> Option<&str> {
        self.api_token.as_deref().map(str::trim).filter(|t| !t.is_empty() && *t != vault::PROTECTED_PLACEHOLDER)
    }

    pub fn producao(&self) -> bool {
        self.environment.as_deref() == Some("producao")
    }

    /// Dados públicos para o cabeçalho de comprovantes (`/establishment`): endereço em uma linha.
    pub fn endereco_resumido(&self) -> Option<String> {
        let junta = |partes: &[&Option<String>], sep: &str| {
            partes.iter().filter_map(|p| p.as_deref().filter(|v| !v.is_empty())).collect::<Vec<_>>().join(sep)
        };
        let linha = [
            junta(&[&self.logradouro, &self.numero], ", "),
            self.bairro.clone().unwrap_or_default(),
            junta(&[&self.municipio, &self.uf], " - "),
        ]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" • ");
        if linha.is_empty() {
            self.address_info.clone().filter(|a| !a.is_empty())
        } else {
            Some(linha)
        }
    }
}

/// Configuração fiscal, ou `None` se ainda não foi salva.
pub async fn ler_config_fiscal(db: &Db) -> DbResult<Option<FiscalSettings>> {
    let row: Option<FiscalSettings> = sqlx::query_as(r#"SELECT * FROM "FiscalSettings" WHERE "id" = ?"#)
        .bind(CONFIG_ID)
        .fetch_optional(db.pool())
        .await?;
    Ok(row.map(|mut s| {
        s.api_token = vault::decrypt_opt(s.api_token);
        s.csc_secret = vault::decrypt_opt(s.csc_secret);
        s
    }))
}

/// Configuração fiscal, criando a linha padrão se não existir (como o `GET /settings`).
pub async fn config_fiscal(db: &Db) -> DbResult<FiscalSettings> {
    if let Some(s) = ler_config_fiscal(db).await? {
        return Ok(s);
    }
    let mut tx = db.begin_write().await?;
    sqlx::query(r#"INSERT OR IGNORE INTO "FiscalSettings" ("id", "updatedAt") VALUES (?, ?)"#)
        .bind(CONFIG_ID)
        .bind(DbTime::now())
        .execute(&mut **tx)
        .await?;
    tx.commit().await?;
    Ok(ler_config_fiscal(db).await?.unwrap_or_default())
}

/// Salva a configuração. Token e CSC são gravados cifrados; o texto de "dado protegido" (campo que não
/// abriu na leitura) mantém o valor que já estava no banco em vez de sobrescrevê-lo.
pub async fn salvar_config_fiscal(db: &Db, s: &FiscalSettings) -> DbResult<FiscalSettings> {
    let segredo = |v: &Option<String>| -> Option<Option<String>> {
        match v.as_deref().map(str::trim) {
            Some(vault::PROTECTED_PLACEHOLDER) => None, // mantém o atual
            Some("") => Some(Some(String::new())),
            Some(t) => Some(Some(vault::encrypt_field(t))),
            None => Some(None),
        }
    };
    let token = segredo(&s.api_token);
    let csc = segredo(&s.csc_secret);
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT INTO "FiscalSettings" (
             "id", "apiToken", "cnpj", "ie", "crt", "cscId", "cscSecret", "addressInfo", "environment",
             "serieNfce", "proximoNumeroNfce", "serieNfe", "proximoNumeroNfe",
             "cep", "logradouro", "numero", "bairro", "municipio", "uf", "razaoSocial", "nomeFantasia", "updatedAt")
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, 'homologacao'), COALESCE(?, '1'), COALESCE(?, 1),
                   COALESCE(?, '1'), COALESCE(?, 1), ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT ("id") DO UPDATE SET
             "apiToken" = CASE WHEN ? THEN excluded."apiToken" ELSE "apiToken" END,
             "cnpj" = excluded."cnpj", "ie" = excluded."ie", "crt" = excluded."crt", "cscId" = excluded."cscId",
             "cscSecret" = CASE WHEN ? THEN excluded."cscSecret" ELSE "cscSecret" END,
             "addressInfo" = excluded."addressInfo", "environment" = excluded."environment",
             "serieNfce" = excluded."serieNfce", "proximoNumeroNfce" = excluded."proximoNumeroNfce",
             "serieNfe" = excluded."serieNfe", "proximoNumeroNfe" = excluded."proximoNumeroNfe",
             "cep" = excluded."cep", "logradouro" = excluded."logradouro", "numero" = excluded."numero",
             "bairro" = excluded."bairro", "municipio" = excluded."municipio", "uf" = excluded."uf",
             "razaoSocial" = excluded."razaoSocial", "nomeFantasia" = excluded."nomeFantasia",
             "updatedAt" = excluded."updatedAt""#,
    )
    .bind(CONFIG_ID)
    .bind(token.clone().flatten())
    .bind(&s.cnpj)
    .bind(&s.ie)
    .bind(&s.crt)
    .bind(&s.csc_id)
    .bind(csc.clone().flatten())
    .bind(&s.address_info)
    .bind(&s.environment)
    .bind(&s.serie_nfce)
    .bind(s.proximo_numero_nfce)
    .bind(&s.serie_nfe)
    .bind(s.proximo_numero_nfe)
    .bind(&s.cep)
    .bind(&s.logradouro)
    .bind(&s.numero)
    .bind(&s.bairro)
    .bind(&s.municipio)
    .bind(&s.uf)
    .bind(&s.razao_social)
    .bind(&s.nome_fantasia)
    .bind(DbTime::now())
    .bind(token.is_some())
    .bind(csc.is_some())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(ler_config_fiscal(db).await?.unwrap_or_default())
}

pub use config_fiscal as get_fiscal_settings;
pub use salvar_config_fiscal as save_fiscal_settings;

pub async fn list_notas_emitidas(
    db: &Db,
    _status: Option<&str>,
    _tipo: Option<&str>,
    _limit: i64,
) -> DbResult<Vec<NotaEmitida>> {
    emitidas_recentes(db, None).await
}

pub async fn month_summary(db: &Db, date: NaiveDate) -> DbResult<Option<ResumoFiscalMes>> {
    resumo_fiscal_mes(db, date.year(), date.month()).await
}

// ============================================================
// Tipo da nota (NFC-e modelo 65 / NF-e modelo 55)
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TipoNota {
    Nfce,
    Nfe,
}

/// NFC-e quando a referência começa com `nfce_`/`cupom_` ou a chave é do modelo 65; todo o resto é NF-e
/// (`isNfce`/`isNfe` do Node; a mesma regra de `barerp_fiscal::modelo_da_nota`).
pub fn tipo_da_nota(referencia: Option<&str>, chave: Option<&str>) -> TipoNota {
    let por_ref = referencia.is_some_and(|r| r.starts_with("nfce_") || r.starts_with("cupom_"));
    let por_chave = chave.is_some_and(|c| c.len() == 44 && c.get(20..22) == Some("65"));
    if por_ref || por_chave {
        TipoNota::Nfce
    } else {
        TipoNota::Nfe
    }
}

// ============================================================
// NotaEmitida
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NotaEmitida {
    pub id: String,
    pub referencia: String,
    pub chave: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    #[sqlx(rename = "dataEmissao")]
    pub data_emissao: Option<String>,
    #[sqlx(rename = "valorTotal")]
    pub valor_total: Option<f64>,
    /// autorizado, processando, erro_autorizacao, cancelado
    pub status: String,
    #[sqlx(rename = "xmlUrl")]
    pub xml_url: Option<String>,
    #[sqlx(rename = "pdfUrl")]
    pub pdf_url: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

impl NotaEmitida {
    pub fn tipo(&self) -> TipoNota {
        tipo_da_nota(Some(&self.referencia), self.chave.as_deref())
    }

    /// Ainda sem retorno definitivo da SEFAZ: vale consultar a Focus de novo.
    pub fn precisa_sincronizar(&self) -> bool {
        self.status == "processando" || self.numero.as_deref().is_none_or(str::is_empty)
    }

    /// Minutos desde a emissão.
    pub fn minutos_desde(&self, agora: DateTime<Utc>) -> i64 {
        (agora - self.created_at.0).num_minutes()
    }
}

/// Dados para gravar uma nota emitida. Campos `None` não apagam o que já está gravado.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistroEmitida {
    pub referencia: String,
    pub chave: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    /// ISO 8601; na criação, `None` grava a hora atual.
    pub data_emissao: Option<String>,
    pub valor_total: Option<f64>,
    /// `None` = "autorizado", como no `persistNfeRecord`.
    pub status: Option<String>,
    pub xml_url: Option<String>,
    pub pdf_url: Option<String>,
}

const COLUNAS_EMITIDA: &str = r#""id", "referencia", "chave", "numero", "serie", "dataEmissao", "valorTotal", "status", "xmlUrl", "pdfUrl", "createdAt""#;

/// Cria ou atualiza a nota pela referência (`persistNfeRecord`). O status sempre é gravado.
pub async fn gravar_emitida(db: &Db, r: &RegistroEmitida) -> DbResult<NotaEmitida> {
    let status = r.status.as_deref().unwrap_or("autorizado");
    let data = r.data_emissao.clone().unwrap_or_else(|| Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    let mut tx = db.begin_write().await?;
    sqlx::query(
        r#"INSERT INTO "NotaEmitida" ("id", "referencia", "chave", "numero", "serie", "dataEmissao", "valorTotal",
                                      "status", "xmlUrl", "pdfUrl", "createdAt")
           VALUES (?, ?, ?, ?, ?, ?, COALESCE(?, 0), ?, ?, ?, ?)
           ON CONFLICT ("referencia") DO UPDATE SET
             "chave" = COALESCE(NULLIF(excluded."chave", ''), "chave"),
             "numero" = COALESCE(NULLIF(excluded."numero", ''), "numero"),
             "serie" = COALESCE(NULLIF(excluded."serie", ''), "serie"),
             "valorTotal" = CASE WHEN ? THEN excluded."valorTotal" ELSE "valorTotal" END,
             "status" = excluded."status",
             "xmlUrl" = COALESCE(NULLIF(excluded."xmlUrl", ''), "xmlUrl"),
             "pdfUrl" = COALESCE(NULLIF(excluded."pdfUrl", ''), "pdfUrl")"#,
    )
    .bind(ids::cuid())
    .bind(&r.referencia)
    .bind(&r.chave)
    .bind(&r.numero)
    .bind(&r.serie)
    .bind(data)
    .bind(r.valor_total)
    .bind(status)
    .bind(&r.xml_url)
    .bind(&r.pdf_url)
    .bind(DbTime::now())
    .bind(r.valor_total.is_some())
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    buscar_emitida_por_referencia(db, &r.referencia)
        .await?
        .ok_or_else(|| DbError::not_found("Nota emitida não encontrada após gravar."))
}

pub async fn buscar_emitida_por_referencia(db: &Db, referencia: &str) -> DbResult<Option<NotaEmitida>> {
    let sql = format!(r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" WHERE "referencia" = ?"#);
    Ok(sqlx::query_as(&sql).bind(referencia).fetch_optional(db.pool()).await?)
}

pub async fn buscar_emitida_por_chave(db: &Db, chave: &str) -> DbResult<Option<NotaEmitida>> {
    let sql = format!(r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" WHERE "chave" = ? ORDER BY "createdAt" DESC LIMIT 1"#);
    Ok(sqlx::query_as(&sql).bind(chave.trim()).fetch_optional(db.pool()).await?)
}

/// Nota por referência, chave ou id (o que a tela tiver em mãos; usado no DANFE e no cancelamento).
pub async fn buscar_emitida(db: &Db, alvo: &str) -> DbResult<Option<NotaEmitida>> {
    let alvo = alvo.trim();
    let sql = format!(
        r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida"
           WHERE "referencia" = ?1 OR "chave" = ?1 OR "id" = ?1
           ORDER BY ("referencia" = ?1) DESC, "createdAt" DESC LIMIT 1"#
    );
    Ok(sqlx::query_as(&sql).bind(alvo).fetch_optional(db.pool()).await?)
}

/// Notas com esse número (aceita "1", "000001" ou "000000001"), opcionalmente da série, mais recentes
/// primeiro. Quem chama escolhe pelo [`NotaEmitida::tipo`] (reimpressão de NFC-e ou de NF-e).
pub async fn buscar_emitidas_por_numero(db: &Db, numero: &str, serie: Option<&str>) -> DbResult<Vec<NotaEmitida>> {
    let numero = numero.trim();
    let (p6, p9) = match numero.parse::<u64>() {
        Ok(n) => (format!("{n:06}"), format!("{n:09}")),
        Err(_) => (numero.to_string(), numero.to_string()),
    };
    let sem_zeros = numero.parse::<u64>().map(|n| n.to_string()).unwrap_or_else(|_| numero.to_string());
    let serie = serie.map(str::trim).filter(|s| !s.is_empty());
    let sql = format!(
        r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida"
           WHERE "numero" IN (?, ?, ?, ?) AND (? IS NULL OR "serie" = ?)
           ORDER BY "createdAt" DESC"#
    );
    Ok(sqlx::query_as(&sql)
        .bind(numero)
        .bind(&sem_zeros)
        .bind(&p6)
        .bind(&p9)
        .bind(serie)
        .bind(serie)
        .fetch_all(db.pool())
        .await?)
}

/// As 50 notas mais recentes (entre as 100 últimas) do tipo pedido, como o `/recent-notes`.
pub async fn emitidas_recentes(db: &Db, tipo: Option<TipoNota>) -> DbResult<Vec<NotaEmitida>> {
    let sql = format!(r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" ORDER BY "createdAt" DESC LIMIT 100"#);
    let notas: Vec<NotaEmitida> = sqlx::query_as(&sql).fetch_all(db.pool()).await?;
    Ok(notas.into_iter().filter(|n| tipo.is_none_or(|t| n.tipo() == t)).take(50).collect())
}

/// Notas autorizadas ainda dentro do prazo de cancelamento: NFC-e 30 minutos, NF-e 24 horas.
/// Devolve a nota e os minutos que faltam.
pub async fn emitidas_cancelaveis(db: &Db, tipo: TipoNota, agora: DateTime<Utc>) -> DbResult<Vec<(NotaEmitida, i64)>> {
    let (limite, prazo) = match tipo {
        TipoNota::Nfce => (100, 30),
        TipoNota::Nfe => (200, 24 * 60),
    };
    let sql = format!(
        r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" WHERE "status" = 'autorizado' ORDER BY "createdAt" DESC LIMIT ?"#
    );
    let notas: Vec<NotaEmitida> = sqlx::query_as(&sql).bind(limite).fetch_all(db.pool()).await?;
    Ok(notas
        .into_iter()
        .filter(|n| n.tipo() == tipo)
        .filter_map(|n| {
            let restam = prazo - n.minutos_desde(agora);
            (restam > 0).then_some((n, restam))
        })
        .collect())
}

/// Marca como cancelada a nota da referência e as demais com a mesma chave.
pub async fn marcar_emitida_cancelada(db: &Db, referencia: &str, chave: Option<&str>) -> DbResult<u64> {
    let mut tx = db.begin_write().await?;
    let r = sqlx::query(
        r#"UPDATE "NotaEmitida" SET "status" = 'cancelado'
           WHERE "referencia" = ? OR (? IS NOT NULL AND ? != '' AND "chave" = ?)"#,
    )
    .bind(referencia)
    .bind(chave)
    .bind(chave)
    .bind(chave)
    .execute(&mut **tx)
    .await?;
    tx.commit().await?;
    Ok(r.rows_affected())
}

/// Atualiza uma nota que já existe (sem criar). Campos `None` ou vazios ficam como estão; o status só
/// muda se vier. Devolve se havia a nota.
pub async fn atualizar_emitida(db: &Db, r: &RegistroEmitida) -> DbResult<bool> {
    let mut tx = db.begin_write().await?;
    let n = sqlx::query(
        r#"UPDATE "NotaEmitida" SET
             "chave" = COALESCE(NULLIF(?, ''), "chave"),
             "numero" = COALESCE(NULLIF(?, ''), "numero"),
             "serie" = COALESCE(NULLIF(?, ''), "serie"),
             "dataEmissao" = COALESCE(NULLIF(?, ''), "dataEmissao"),
             "valorTotal" = COALESCE(?, "valorTotal"),
             "status" = COALESCE(NULLIF(?, ''), "status"),
             "xmlUrl" = COALESCE(NULLIF(?, ''), "xmlUrl"),
             "pdfUrl" = COALESCE(NULLIF(?, ''), "pdfUrl")
           WHERE "referencia" = ?"#,
    )
    .bind(&r.chave)
    .bind(&r.numero)
    .bind(&r.serie)
    .bind(&r.data_emissao)
    .bind(r.valor_total)
    .bind(&r.status)
    .bind(&r.xml_url)
    .bind(&r.pdf_url)
    .bind(&r.referencia)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(n > 0)
}

/// Notas do tipo ainda sem retorno definitivo (processando ou sem número), mais recentes primeiro, para
/// consultar a Focus de novo.
pub async fn emitidas_para_sincronizar(db: &Db, tipo: TipoNota, limite: usize) -> DbResult<Vec<NotaEmitida>> {
    let sql = format!(
        r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida"
           WHERE "status" = 'processando' OR "numero" IS NULL OR "numero" = ''
           ORDER BY "createdAt" DESC"#
    );
    let notas: Vec<NotaEmitida> = sqlx::query_as(&sql).fetch_all(db.pool()).await?;
    Ok(notas.into_iter().filter(|n| n.tipo() == tipo).take(limite).collect())
}

/// Notas cuja chave contém o trecho (ex.: o número com 9 dígitos), mais recentes primeiro.
pub async fn buscar_emitidas_chave_contendo(db: &Db, trecho: &str) -> DbResult<Vec<NotaEmitida>> {
    let sql = format!(r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" WHERE instr("chave", ?) > 0 ORDER BY "createdAt" DESC"#);
    Ok(sqlx::query_as(&sql).bind(trecho).fetch_all(db.pool()).await?)
}

/// Série e numeração de um modelo: o próximo número configurado (enviado à Focus) e o maior número
/// já emitido nessa série, para conferir buracos antes de inutilizar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Numeracao {
    pub serie: String,
    pub proximo_configurado: i64,
    pub ultimo_emitido: Option<i64>,
}

pub async fn numeracao(db: &Db, tipo: TipoNota) -> DbResult<Numeracao> {
    let cfg = ler_config_fiscal(db).await?.unwrap_or_default();
    let (serie, proximo) = match tipo {
        TipoNota::Nfce => (cfg.serie_nfce, cfg.proximo_numero_nfce),
        TipoNota::Nfe => (cfg.serie_nfe, cfg.proximo_numero_nfe),
    };
    let serie = serie.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "1".into());
    let rows: Vec<(String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"SELECT "referencia", "chave", "numero" FROM "NotaEmitida"
           WHERE "serie" = ? AND "numero" IS NOT NULL AND "status" IN ('autorizado', 'cancelado')"#,
    )
    .bind(serie.trim())
    .fetch_all(db.pool())
    .await?;
    let ultimo = rows
        .iter()
        .filter(|(r, c, _)| tipo_da_nota(Some(r), c.as_deref()) == tipo)
        .filter_map(|(_, _, n)| n.as_deref()?.trim().parse::<i64>().ok())
        .max();
    Ok(Numeracao { serie, proximo_configurado: proximo.unwrap_or(1), ultimo_emitido: ultimo })
}

// ============================================================
// NotaRecebida
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct NotaRecebida {
    pub id: String,
    pub chave: String,
    pub emitente: Option<String>,
    #[sqlx(rename = "cnpjEmitente")]
    pub cnpj_emitente: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    #[sqlx(rename = "dataEmissao")]
    pub data_emissao: Option<String>,
    #[sqlx(rename = "valorTotal")]
    pub valor_total: Option<f64>,
    /// pendente (aguardando o 1º bip), recebida, finalizada (entrada no estoque feita)
    pub status: String,
    #[sqlx(rename = "xmlContent")]
    pub xml_content: Option<String>,
    #[sqlx(rename = "createdAt")]
    pub created_at: DbTime,
}

impl NotaRecebida {
    /// Já tem o XML completo (e não só o resumo).
    pub fn tem_xml(&self) -> bool {
        self.xml_content.as_deref().is_some_and(xml_completo)
    }

    /// Valor da nota; se estiver zerado, o `vNF` do XML salvo (`valorRecebidaComFallback`).
    pub fn valor(&self) -> f64 {
        valor_com_fallback(self.valor_total, self.xml_content.as_deref())
    }
}

fn xml_completo(xml: &str) -> bool {
    xml.len() > 50 && (xml.contains("<nfeProc") || xml.contains("<NFe") || xml.starts_with("<?xml"))
}

/// `vNF` do XML (do `ICMSTot` da nota completa ou do resumo `resNFe`). Aceita o XML guardado como
/// `{"raw": "..."}`, formato antigo de algumas linhas.
pub fn vnf_do_xml(xml: &str) -> Option<f64> {
    let raw;
    let xml = if xml.trim_start().starts_with('{') {
        raw = serde_json::from_str::<serde_json::Value>(xml).ok()?.get("raw")?.as_str()?.to_string();
        raw.as_str()
    } else {
        xml
    };
    let trecho = |inicio: &str| xml.find(inicio).map(|i| &xml[i..]);
    let base = trecho("<ICMSTot").or_else(|| trecho("<resNFe"))?;
    let ini = base.find("<vNF>")? + "<vNF>".len();
    let fim = base[ini..].find("</vNF>")?;
    base[ini..ini + fim].trim().parse::<f64>().ok().filter(|v| *v > 0.0)
}

fn valor_com_fallback(valor: Option<f64>, xml: Option<&str>) -> f64 {
    let v = valor.unwrap_or(0.0);
    if v > 0.0 {
        return v;
    }
    xml.and_then(vnf_do_xml).unwrap_or(v)
}

fn so_digitos(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// Dados de uma nota recebida (da sincronização com a Focus, do bip da chave ou da importação do XML).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistroRecebida {
    pub chave: String,
    pub emitente: Option<String>,
    pub cnpj_emitente: Option<String>,
    pub numero: Option<String>,
    pub serie: Option<String>,
    pub data_emissao: Option<String>,
    pub valor_total: Option<f64>,
    pub xml_content: Option<String>,
}

const COLUNAS_RECEBIDA: &str = r#""id", "chave", "emitente", "cnpjEmitente", "numero", "serie", "dataEmissao", "valorTotal", "status", "xmlContent", "createdAt""#;

/// Chaves já importadas no estoque (`PriceHistory.nfeChave`), só dígitos.
async fn chaves_importadas(conn: &mut SqliteConnection) -> DbResult<std::collections::HashSet<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT DISTINCT "nfeChave" FROM "PriceHistory" WHERE "nfeChave" IS NOT NULL AND TRIM("nfeChave") != ''"#,
    )
    .fetch_all(conn)
    .await?;
    Ok(rows.into_iter().map(|(c,)| so_digitos(&c)).collect())
}

/// Cria ou atualiza a nota pela chave e devolve se foi criada.
///
/// - Nota nova entra com `status` (padrão "pendente"), ou "finalizada" se a chave já foi importada no estoque.
/// - Nota existente: "finalizada" nunca volta atrás; `status = None` mantém o atual. Texto vazio e valor 0
///   não apagam o que já estava gravado, e o XML só é trocado quando vem um novo.
/// - Valor zerado é preenchido pelo `vNF` do XML.
pub async fn gravar_recebida(db: &Db, r: &RegistroRecebida, status: Option<&str>) -> DbResult<bool> {
    let chave = so_digitos(&r.chave);
    if chave.is_empty() {
        return Err(DbError::rule("Chave de acesso inválida."));
    }
    let valor = valor_com_fallback(r.valor_total, r.xml_content.as_deref());
    let mut tx = db.begin_write().await?;
    let importada = chaves_importadas(&mut tx).await?.contains(&chave);
    let existente = recebida_por_chave(&mut tx, &chave).await?;
    let criada = existente.is_none();
    let status_novo = match (&existente, status) {
        (Some(atual), _) if atual.status == "finalizada" => "finalizada".to_string(),
        (_, _) if importada => "finalizada".to_string(),
        (Some(atual), None) => atual.status.clone(),
        (None, None) => "pendente".to_string(),
        (_, Some(s)) => s.to_string(),
    };
    let cnpj = r.cnpj_emitente.as_deref().map(so_digitos);
    sqlx::query(
        r#"INSERT INTO "NotaRecebida" ("id", "chave", "emitente", "cnpjEmitente", "numero", "serie", "dataEmissao",
                                       "valorTotal", "status", "xmlContent", "createdAt")
           VALUES (?1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
           ON CONFLICT ("chave") DO UPDATE SET
             "emitente" = COALESCE(NULLIF(excluded."emitente", ''), "emitente"),
             "cnpjEmitente" = COALESCE(NULLIF(excluded."cnpjEmitente", ''), "cnpjEmitente"),
             "numero" = COALESCE(NULLIF(excluded."numero", ''), "numero"),
             "serie" = COALESCE(NULLIF(excluded."serie", ''), "serie"),
             "dataEmissao" = COALESCE(NULLIF(excluded."dataEmissao", ''), "dataEmissao"),
             "valorTotal" = CASE WHEN excluded."valorTotal" > 0 THEN excluded."valorTotal" ELSE "valorTotal" END,
             "status" = excluded."status",
             "xmlContent" = COALESCE(NULLIF(excluded."xmlContent", ''), "xmlContent")"#,
    )
    .bind(&chave)
    .bind(&r.emitente)
    .bind(&cnpj)
    .bind(&r.numero)
    .bind(&r.serie)
    .bind(&r.data_emissao)
    .bind(valor)
    .bind(&status_novo)
    .bind(&r.xml_content)
    .bind(DbTime::now())
    .execute(&mut **tx)
    .await?;
    // nota antiga com valor zerado e XML já salvo: corrige pelo vNF
    if let Some(n) = recebida_por_chave(&mut tx, &chave).await? {
        if n.valor_total.unwrap_or(0.0) <= 0.0 && n.valor() > 0.0 {
            sqlx::query(r#"UPDATE "NotaRecebida" SET "valorTotal" = ? WHERE "chave" = ?"#)
                .bind(n.valor())
                .bind(&chave)
                .execute(&mut **tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(criada)
}

/// Nota pela chave (aceita a chave com espaços ou pontuação, e linhas antigas gravadas com espaços).
pub async fn buscar_recebida(db: &Db, chave: &str) -> DbResult<Option<NotaRecebida>> {
    recebida_por_chave(&mut *db.pool().acquire().await?, chave).await
}

async fn recebida_por_chave(conn: &mut SqliteConnection, chave: &str) -> DbResult<Option<NotaRecebida>> {
    let chave = so_digitos(chave);
    let sql = format!(
        r#"SELECT {COLUNAS_RECEBIDA} FROM "NotaRecebida" WHERE "chave" = ?1 OR REPLACE("chave", ' ', '') = ?1 LIMIT 1"#
    );
    Ok(sqlx::query_as(&sql).bind(chave).fetch_optional(conn).await?)
}

/// As 50 mais recentes (`/notas-recebidas`).
pub async fn recebidas_recentes(db: &Db) -> DbResult<Vec<NotaRecebida>> {
    let sql = format!(r#"SELECT {COLUNAS_RECEBIDA} FROM "NotaRecebida" ORDER BY "createdAt" DESC LIMIT 50"#);
    Ok(sqlx::query_as(&sql).fetch_all(db.pool()).await?)
}

async fn mudar_status(db: &Db, chave: &str, sql_status: &str, xml: Option<&str>) -> DbResult<u64> {
    let chave = so_digitos(chave);
    let sql = format!(
        r#"UPDATE "NotaRecebida" SET "status" = {sql_status}, "xmlContent" = COALESCE(?1, "xmlContent")
           WHERE "chave" = ?2 OR REPLACE("chave", ' ', '') = ?2"#
    );
    let mut tx = db.begin_write().await?;
    let n = sqlx::query(&sql).bind(xml).bind(chave).execute(&mut **tx).await?.rows_affected();
    tx.commit().await?;
    Ok(n)
}

/// 1º bip: a mercadoria chegou ("recebida", sem desfazer "finalizada"), guardando o XML se veio.
pub async fn receber_recebida(db: &Db, chave: &str, xml: Option<&str>) -> DbResult<u64> {
    mudar_status(db, chave, "CASE WHEN \"status\" = 'finalizada' THEN 'finalizada' ELSE 'recebida' END", xml).await
}

/// Entrada no estoque feita.
pub async fn finalizar_recebida(db: &Db, chave: &str) -> DbResult<u64> {
    mudar_status(db, chave, "'finalizada'", None).await
}

/// Volta para conferência (2º bip).
pub async fn reabrir_recebida(db: &Db, chave: &str) -> DbResult<u64> {
    mudar_status(db, chave, "'recebida'", None).await
}

/// Linha da tela de notas de entrada (`/notas-pendentes`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotaRecebidaResumo {
    pub id: String,
    pub chave: String,
    pub emitente: String,
    pub cnpj_emitente: String,
    pub numero: String,
    pub serie: String,
    /// `dataEmissao`, ou a data de cadastro quando a nota não tem.
    pub data_emissao: String,
    pub valor_total: f64,
    /// pendente, recebida ou finalizada
    pub status: String,
    pub has_xml: bool,
    pub created_at: DbTime,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContagemRecebidas {
    pub total: usize,
    pub pendentes: usize,
    pub recebidas: usize,
    pub finalizadas: usize,
    pub com_xml: usize,
    pub sem_xml: usize,
}

/// Filtros da tela; `None` ou "todos" não filtra.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiltroRecebidas {
    pub status: Option<String>,
    /// com_xml ou sem_xml
    pub xml_status: Option<String>,
    /// AAAA-MM-DD
    pub data_inicio: Option<String>,
    pub data_fim: Option<String>,
    /// Trecho do emitente, CNPJ, chave ou número.
    pub fornecedor: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ListaRecebidas {
    pub notas: Vec<NotaRecebidaResumo>,
    /// Contagem de todas as notas, antes dos filtros.
    pub summary: ContagemRecebidas,
}

/// Tela de notas de entrada: corrige o status das já importadas no estoque, normaliza status antigos
/// e aplica os filtros.
pub async fn listar_recebidas(db: &Db, filtro: &FiltroRecebidas) -> DbResult<ListaRecebidas> {
    let importadas = chaves_importadas(&mut *db.pool().acquire().await?).await?;
    let sql = format!(r#"SELECT {COLUNAS_RECEBIDA} FROM "NotaRecebida" ORDER BY "createdAt" DESC, "dataEmissao" DESC"#);
    let linhas: Vec<NotaRecebida> = sqlx::query_as(&sql).fetch_all(db.pool()).await?;

    let mut corrigir = Vec::new();
    let todas: Vec<NotaRecebidaResumo> = linhas
        .into_iter()
        .map(|n| {
            let mut status = n.status.to_lowercase();
            if importadas.contains(&so_digitos(&n.chave)) {
                if n.status != "finalizada" {
                    corrigir.push(n.id.clone());
                }
                status = "finalizada".into();
            }
            match status.as_str() {
                "importada" => status = "finalizada".into(),
                "ciencia_registrada" | "" => status = "pendente".into(),
                _ => {}
            }
            let has_xml = n.tem_xml();
            NotaRecebidaResumo {
                valor_total: valor_com_fallback(n.valor_total, if has_xml { n.xml_content.as_deref() } else { None }),
                data_emissao: n
                    .data_emissao
                    .clone()
                    .filter(|d| !d.is_empty())
                    .unwrap_or_else(|| n.created_at.0.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
                id: n.id,
                chave: n.chave,
                emitente: n.emitente.filter(|e| !e.is_empty()).unwrap_or_else(|| "Fornecedor".into()),
                cnpj_emitente: n.cnpj_emitente.unwrap_or_default(),
                numero: n.numero.unwrap_or_default(),
                serie: n.serie.unwrap_or_default(),
                status,
                has_xml,
                created_at: n.created_at,
            }
        })
        .collect();

    // auto-correção: nota já importada no estoque fica finalizada no banco também
    if !corrigir.is_empty() {
        let mut tx = db.begin_write().await?;
        for id in corrigir {
            sqlx::query(r#"UPDATE "NotaRecebida" SET "status" = 'finalizada' WHERE "id" = ?"#)
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
        tx.commit().await?;
    }

    let conta = |st: &str| todas.iter().filter(|n| n.status == st).count();
    let com_xml = todas.iter().filter(|n| n.has_xml).count();
    let summary = ContagemRecebidas {
        total: todas.len(),
        pendentes: conta("pendente"),
        recebidas: conta("recebida"),
        finalizadas: conta("finalizada"),
        com_xml,
        sem_xml: todas.len() - com_xml,
    };

    let ativo = |v: &Option<String>| v.as_deref().map(str::trim).filter(|v| !v.is_empty() && *v != "todos").map(str::to_string);
    let status = ativo(&filtro.status).map(|s| s.to_lowercase());
    let xml = ativo(&filtro.xml_status);
    let ini = ativo(&filtro.data_inicio);
    let fim = ativo(&filtro.data_fim);
    let busca = ativo(&filtro.fornecedor).map(|f| f.to_lowercase());
    let notas = todas
        .into_iter()
        .filter(|n| status.as_ref().is_none_or(|s| &n.status == s))
        .filter(|n| match xml.as_deref() {
            Some("com_xml") => n.has_xml,
            Some("sem_xml") => !n.has_xml,
            _ => true,
        })
        .filter(|n| {
            busca.as_ref().is_none_or(|q| {
                n.emitente.to_lowercase().contains(q.as_str())
                    || n.cnpj_emitente.contains(q.as_str())
                    || n.chave.contains(q.as_str())
                    || n.numero.contains(q.as_str())
            })
        })
        .filter(|n| {
            let dia: String = n.data_emissao.chars().take(10).collect();
            ini.as_ref().is_none_or(|i| dia >= *i) && fim.as_ref().is_none_or(|f| dia <= *f)
        })
        .collect();
    Ok(ListaRecebidas { notas, summary })
}

// ============================================================
// Resumo fiscal do mês
// ============================================================

/// Parte fiscal do `/month-summary` (a posição de estoque fica com o módulo de produtos).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumoFiscalMes {
    /// AAAA-MM
    pub month: String,
    pub emitidas_count: usize,
    pub emitidas_total: f64,
    pub nfce_count: usize,
    pub nfce_total: f64,
    pub nfe_count: usize,
    pub nfe_total: f64,
    pub recebidas_count: usize,
    pub recebidas_total: f64,
    pub has_notes: bool,
}

/// Notas do mês (mais antigas primeiro): entra a nota com `dataEmissao` começando por AAAA-MM ou cadastrada
/// no mês (UTC, como no Node). `None` se o mês for inválido.
pub async fn notas_do_mes(db: &Db, ano: i32, mes: u32) -> DbResult<Option<(Vec<NotaEmitida>, Vec<NotaRecebida>)>> {
    let Some(inicio) = NaiveDate::from_ymd_opt(ano, mes, 1) else {
        return Ok(None);
    };
    let fim = if mes == 12 { NaiveDate::from_ymd_opt(ano + 1, 1, 1) } else { NaiveDate::from_ymd_opt(ano, mes + 1, 1) };
    let (Some(ini), Some(fim)) = (inicio.and_hms_opt(0, 0, 0), fim.and_then(|f| f.and_hms_opt(0, 0, 0))) else {
        return Ok(None);
    };
    let (ini, fim) = (Utc.from_utc_datetime(&ini), Utc.from_utc_datetime(&fim));
    let prefixo = format!("{ano:04}-{mes:02}");
    let no_mes = |data: &Option<String>, criada: &DbTime| {
        data.as_deref().is_some_and(|d| d.starts_with(&prefixo)) || (criada.0 >= ini && criada.0 < fim)
    };
    let sql = format!(r#"SELECT {COLUNAS_EMITIDA} FROM "NotaEmitida" ORDER BY "createdAt" ASC"#);
    let emitidas: Vec<NotaEmitida> = sqlx::query_as(&sql).fetch_all(db.pool()).await?;
    let sql = format!(r#"SELECT {COLUNAS_RECEBIDA} FROM "NotaRecebida" ORDER BY "createdAt" ASC"#);
    let recebidas: Vec<NotaRecebida> = sqlx::query_as(&sql).fetch_all(db.pool()).await?;
    Ok(Some((
        emitidas.into_iter().filter(|n| no_mes(&n.data_emissao, &n.created_at)).collect(),
        recebidas.into_iter().filter(|n| no_mes(&n.data_emissao, &n.created_at)).collect(),
    )))
}

/// Resumo de [`notas_do_mes`].
pub async fn resumo_fiscal_mes(db: &Db, ano: i32, mes: u32) -> DbResult<Option<ResumoFiscalMes>> {
    let Some((emitidas, recebidas)) = notas_do_mes(db, ano, mes).await? else {
        return Ok(None);
    };
    let prefixo = format!("{ano:04}-{mes:02}");
    let soma = |it: &mut dyn Iterator<Item = &NotaEmitida>| it.map(|n| n.valor_total.unwrap_or(0.0)).sum::<f64>();
    let nfce: Vec<&NotaEmitida> = emitidas.iter().filter(|n| n.tipo() == TipoNota::Nfce).collect();
    let nfe: Vec<&NotaEmitida> = emitidas.iter().filter(|n| n.tipo() == TipoNota::Nfe).collect();
    Ok(Some(ResumoFiscalMes {
        month: prefixo,
        emitidas_count: emitidas.len(),
        emitidas_total: soma(&mut emitidas.iter()),
        nfce_count: nfce.len(),
        nfce_total: soma(&mut nfce.iter().copied()),
        nfe_count: nfe.len(),
        nfe_total: soma(&mut nfe.iter().copied()),
        recebidas_count: recebidas.len(),
        recebidas_total: recebidas.iter().map(NotaRecebida::valor).sum(),
        has_notes: !emitidas.is_empty() || !recebidas.is_empty(),
    }))
}

// ============================================================
// Cofre de XML em disco (guarda por 5 anos)
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadoCofre {
    /// Notas emitidas pelo bar.
    Saida,
    /// Notas de compra.
    Entrada,
}

impl LadoCofre {
    fn pasta(self) -> &'static str {
        match self {
            LadoCofre::Saida => "SAIDA",
            LadoCofre::Entrada => "ENTRADA",
        }
    }
}

/// `<base>/AAAA/MM/SAIDA|ENTRADA/<chave>.xml`. A chave vira só letras, dígitos, `_` e `-` (nada de `..`
/// ou barras vindas de fora); `None` se não sobrar nada.
pub fn caminho_cofre(base: &Path, lado: LadoCofre, chave: &str, ano: i32, mes: u32) -> Option<PathBuf> {
    let nome: String = chave.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    if nome.is_empty() {
        return None;
    }
    Some(base.join(format!("{ano:04}")).join(format!("{mes:02}")).join(lado.pasta()).join(format!("{nome}.xml")))
}

/// Grava o XML no cofre na pasta do mês atual (horário local, como o Node) e devolve o caminho.
pub fn arquivar_xml(base: &Path, lado: LadoCofre, chave: &str, xml: &str) -> io::Result<PathBuf> {
    let hoje = Local::now();
    let caminho = caminho_cofre(base, lado, chave, hoje.year(), hoje.month())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "chave de acesso vazia"))?;
    if let Some(pasta) = caminho.parent() {
        std::fs::create_dir_all(pasta)?;
    }
    std::fs::write(&caminho, xml)?;
    Ok(caminho)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    const CHAVE_55: &str = "15260936275163000124550020000000011794431341";
    const CHAVE_65: &str = "15260936275163000124650010000000071794431341";

    fn xml_com_vnf(v: &str) -> String {
        format!("<?xml version=\"1.0\"?><nfeProc><NFe><infNFe><total><ICMSTot><vNF>{v}</vNF></ICMSTot></total></infNFe></NFe></nfeProc>")
    }

    #[tokio::test]
    async fn config_fiscal_cifra_e_mantem_segredo_protegido() {
        let db = Db::open_in_memory().await.unwrap();
        assert_eq!(ler_config_fiscal(&db).await.unwrap(), None);
        let padrao = config_fiscal(&db).await.unwrap();
        assert_eq!(padrao.environment.as_deref(), Some("homologacao"));
        assert_eq!(padrao.serie_nfce.as_deref(), Some("1"));
        assert_eq!(padrao.proximo_numero_nfe, Some(1));

        let mut s = padrao.clone();
        s.api_token = Some("  tok-123 ".into());
        s.csc_secret = Some("CSC".into());
        s.cnpj = Some("36.275.163/0001-24".into());
        s.crt = Some("1".into());
        s.serie_nfe = Some("2".into());
        s.logradouro = Some("Av. Brasil".into());
        s.numero = Some("100".into());
        s.municipio = Some("Belém".into());
        s.uf = Some("PA".into());
        let salvo = salvar_config_fiscal(&db, &s).await.unwrap();
        assert_eq!(salvo.token(), Some("tok-123"));
        assert_eq!(salvo.csc_secret.as_deref(), Some("CSC"));

        let (bruto,): (String,) = sqlx::query_as(r#"SELECT "apiToken" FROM "FiscalSettings""#).fetch_one(db.pool()).await.unwrap();
        assert!(bruto.starts_with("$enc$v1$"));

        // o texto de dado protegido não sobrescreve o token que já estava gravado
        let mut s2 = salvo.clone();
        s2.api_token = Some(vault::PROTECTED_PLACEHOLDER.into());
        s2.csc_secret = None;
        let salvo2 = salvar_config_fiscal(&db, &s2).await.unwrap();
        assert_eq!(salvo2.token(), Some("tok-123"));
        assert_eq!(salvo2.csc_secret, None);

        let e = salvo2.emitente();
        assert_eq!((e.cnpj.as_str(), e.serie_nfe.as_str(), e.serie_nfce.as_str()), ("36.275.163/0001-24", "2", "1"));
        assert_eq!(salvo2.endereco_resumido().as_deref(), Some("Av. Brasil, 100 • Belém - PA"));
        assert!(!salvo2.producao());
    }

    #[test]
    fn tipo() {
        assert_eq!(tipo_da_nota(Some("nfce_x"), None), TipoNota::Nfce);
        assert_eq!(tipo_da_nota(Some("x"), Some(CHAVE_65)), TipoNota::Nfce);
        assert_eq!(tipo_da_nota(Some("nfe_1"), Some(CHAVE_55)), TipoNota::Nfe);
        assert_eq!(tipo_da_nota(None, None), TipoNota::Nfe);
    }

    #[tokio::test]
    async fn nota_emitida_ciclo() {
        let db = Db::open_in_memory().await.unwrap();
        // processando: só referência e valor
        let n = gravar_emitida(
            &db,
            &RegistroEmitida {
                referencia: "nfe_9_1000".into(),
                valor_total: Some(700.0),
                status: Some("processando".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!((n.status.as_str(), n.valor_total, n.numero.as_deref()), ("processando", Some(700.0), None));
        assert!(n.precisa_sincronizar());
        assert!(n.data_emissao.as_deref().unwrap().ends_with('Z'));

        // autorizou: completa sem perder o valor
        let n = gravar_emitida(
            &db,
            &RegistroEmitida {
                referencia: "nfe_9_1000".into(),
                chave: Some(CHAVE_55.into()),
                numero: Some("1".into()),
                serie: Some("2".into()),
                pdf_url: Some("/api/fiscal/danfe/nfe_9_1000".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(n.status, "autorizado");
        assert_eq!(n.valor_total, Some(700.0));
        assert_eq!(n.chave.as_deref(), Some(CHAVE_55));
        assert!(!n.precisa_sincronizar());

        gravar_emitida(&db, &RegistroEmitida { referencia: "nfce_venda1".into(), numero: Some("7".into()), serie: Some("1".into()), valor_total: Some(20.0), ..Default::default() })
            .await
            .unwrap();

        assert_eq!(buscar_emitida(&db, CHAVE_55).await.unwrap().unwrap().referencia, "nfe_9_1000");
        assert_eq!(buscar_emitida(&db, &n.id).await.unwrap().unwrap().referencia, "nfe_9_1000");
        assert!(buscar_emitida(&db, "nada").await.unwrap().is_none());
        assert_eq!(buscar_emitida_por_chave(&db, CHAVE_55).await.unwrap().unwrap().numero.as_deref(), Some("1"));

        let por_numero = buscar_emitidas_por_numero(&db, "000000001", None).await.unwrap();
        assert_eq!(por_numero.len(), 1);
        assert!(buscar_emitidas_por_numero(&db, "1", Some("3")).await.unwrap().is_empty());
        assert_eq!(buscar_emitidas_por_numero(&db, "7", Some("1")).await.unwrap()[0].tipo(), TipoNota::Nfce);

        let nfce = emitidas_recentes(&db, Some(TipoNota::Nfce)).await.unwrap();
        assert_eq!(nfce.iter().map(|n| n.referencia.as_str()).collect::<Vec<_>>(), ["nfce_venda1"]);
        assert_eq!(emitidas_recentes(&db, None).await.unwrap().len(), 2);

        let agora = Utc::now();
        assert_eq!(emitidas_cancelaveis(&db, TipoNota::Nfce, agora).await.unwrap()[0].1, 30);
        assert!(emitidas_cancelaveis(&db, TipoNota::Nfce, agora + Duration::minutes(31)).await.unwrap().is_empty());
        assert_eq!(emitidas_cancelaveis(&db, TipoNota::Nfe, agora + Duration::hours(2)).await.unwrap()[0].1, 22 * 60);

        let num = numeracao(&db, TipoNota::Nfe).await.unwrap();
        assert_eq!(num, Numeracao { serie: "1".into(), proximo_configurado: 1, ultimo_emitido: None });
        let mut cfg = config_fiscal(&db).await.unwrap();
        cfg.serie_nfe = Some("2".into());
        cfg.proximo_numero_nfe = Some(5);
        salvar_config_fiscal(&db, &cfg).await.unwrap();
        assert_eq!(
            numeracao(&db, TipoNota::Nfe).await.unwrap(),
            Numeracao { serie: "2".into(), proximo_configurado: 5, ultimo_emitido: Some(1) }
        );

        assert_eq!(marcar_emitida_cancelada(&db, "outra", Some(CHAVE_55)).await.unwrap(), 1);
        assert_eq!(buscar_emitida(&db, "nfe_9_1000").await.unwrap().unwrap().status, "cancelado");
        assert_eq!(marcar_emitida_cancelada(&db, "nfce_venda1", None).await.unwrap(), 1);
        assert!(emitidas_cancelaveis(&db, TipoNota::Nfce, agora).await.unwrap().is_empty());
    }

    #[test]
    fn vnf() {
        assert_eq!(vnf_do_xml(&xml_com_vnf("476.00")), Some(476.0));
        assert_eq!(vnf_do_xml("<resNFe><chNFe>1</chNFe><vNF>12.5</vNF></resNFe>"), Some(12.5));
        assert_eq!(vnf_do_xml(r#"{"raw": "<resNFe><vNF>3</vNF></resNFe>"}"#), Some(3.0));
        assert_eq!(vnf_do_xml(&xml_com_vnf("0.00")), None);
        assert_eq!(vnf_do_xml("lixo"), None);
    }

    #[tokio::test]
    async fn nota_recebida_ciclo() {
        let db = Db::open_in_memory().await.unwrap();
        let base = RegistroRecebida {
            chave: CHAVE_55.into(),
            emitente: Some("Fornecedor CNPJ 36.275.163/0001-24".into()),
            cnpj_emitente: Some("36.275.163/0001-24".into()),
            numero: Some("1".into()),
            serie: Some("2".into()),
            data_emissao: Some("2026-09-11T12:00:00-03:00".into()),
            valor_total: Some(0.0),
            xml_content: None,
        };
        assert!(gravar_recebida(&db, &base, None).await.unwrap());
        let n = buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap();
        assert_eq!((n.status.as_str(), n.valor_total, n.cnpj_emitente.as_deref()), ("pendente", Some(0.0), Some("36275163000124")));
        assert!(!n.tem_xml());

        // chegou o XML: valor vem do vNF, nome melhora, status fica
        let com_xml = RegistroRecebida {
            emitente: Some("Distribuidora LTDA".into()),
            numero: Some(String::new()),
            xml_content: Some(xml_com_vnf("476.00")),
            ..base.clone()
        };
        assert!(!gravar_recebida(&db, &com_xml, None).await.unwrap());
        let n = buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap();
        assert_eq!((n.status.as_str(), n.valor_total, n.numero.as_deref()), ("pendente", Some(476.0), Some("1")));
        assert_eq!(n.emitente.as_deref(), Some("Distribuidora LTDA"));
        assert!(n.tem_xml());

        assert_eq!(receber_recebida(&db, "1526 0936 2751 6300 0124 5500 2000 0000 0117 9443 1341", None).await.unwrap(), 1);
        assert_eq!(buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap().status, "recebida");
        finalizar_recebida(&db, CHAVE_55).await.unwrap();
        receber_recebida(&db, CHAVE_55, None).await.unwrap();
        // finalizada não volta atrás, nem pelo bip nem por nova sincronização
        gravar_recebida(&db, &base, Some("recebida")).await.unwrap();
        let n = buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap();
        assert_eq!((n.status.as_str(), n.valor_total), ("finalizada", Some(476.0)));
        reabrir_recebida(&db, CHAVE_55).await.unwrap();
        assert_eq!(buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap().status, "recebida");
        assert_eq!(recebidas_recentes(&db).await.unwrap().len(), 1);
    }

    async fn importar_no_estoque(db: &Db, chave: &str) {
        sqlx::query(r#"INSERT INTO "Category" ("id", "name") VALUES ('c', 'Bebidas')"#)
            .execute(db.pool())
            .await
            .unwrap();
        sqlx::query(
            r#"INSERT INTO "Product" ("id", "name", "price", "categoryId", "createdAt", "updatedAt") VALUES ('p', 'X', 1, 'c', 0, 0)"#,
        )
        .execute(db.pool())
        .await
        .unwrap();
        sqlx::query(
            r#"INSERT INTO "PriceHistory" ("id", "productId", "oldPrice", "newPrice", "priceDiff", "pricePercent", "reason", "nfeChave", "createdAt")
               VALUES (?, 'p', 1, 1, 0, 0, 'NF-e', ?, 0)"#,
        )
        .bind(ids::cuid())
        .bind(chave)
        .execute(db.pool())
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn lista_de_entrada_e_resumo_do_mes() {
        let db = Db::open_in_memory().await.unwrap();
        let nota = |chave: &str, data: &str, valor: f64| RegistroRecebida {
            chave: chave.into(),
            emitente: Some("Ambev".into()),
            data_emissao: Some(data.into()),
            valor_total: Some(valor),
            ..Default::default()
        };
        let outra = "15261036275163000124550020000000021794431341";
        let terceira = "15260836275163000124550020000000031794431341";
        gravar_recebida(&db, &nota(CHAVE_55, "2026-09-11T12:00:00-03:00", 100.0), None).await.unwrap();
        gravar_recebida(&db, &RegistroRecebida { xml_content: Some(xml_com_vnf("50")), ..nota(outra, "2026-10-01", 0.0) }, Some("recebida"))
            .await
            .unwrap();
        gravar_recebida(&db, &nota(terceira, "2026-08-30", 10.0), None).await.unwrap();
        sqlx::query(r#"UPDATE "NotaRecebida" SET "status" = 'ciencia_registrada' WHERE "chave" = ?"#)
            .bind(terceira)
            .execute(db.pool())
            .await
            .unwrap();
        // importada no estoque por fora: a listagem corrige para finalizada
        importar_no_estoque(&db, &format!(" {CHAVE_55} ")).await;

        let tudo = listar_recebidas(&db, &FiltroRecebidas::default()).await.unwrap();
        assert_eq!(
            tudo.summary,
            ContagemRecebidas { total: 3, pendentes: 1, recebidas: 1, finalizadas: 1, com_xml: 1, sem_xml: 2 }
        );
        assert_eq!(buscar_recebida(&db, CHAVE_55).await.unwrap().unwrap().status, "finalizada");
        let r = tudo.notas.iter().find(|n| n.chave == outra).unwrap();
        assert_eq!((r.valor_total, r.has_xml), (50.0, true));

        let f = |status: Option<&str>, xml: Option<&str>, ini: Option<&str>, fim: Option<&str>, forn: Option<&str>| FiltroRecebidas {
            status: status.map(Into::into),
            xml_status: xml.map(Into::into),
            data_inicio: ini.map(Into::into),
            data_fim: fim.map(Into::into),
            fornecedor: forn.map(Into::into),
        };
        let qtd = |l: ListaRecebidas| l.notas.len();
        assert_eq!(qtd(listar_recebidas(&db, &f(Some("pendente"), None, None, None, None)).await.unwrap()), 1);
        assert_eq!(qtd(listar_recebidas(&db, &f(Some("todos"), Some("sem_xml"), None, None, None)).await.unwrap()), 2);
        assert_eq!(qtd(listar_recebidas(&db, &f(None, None, Some("2026-09-01"), Some("2026-09-30"), None)).await.unwrap()), 1);
        assert_eq!(qtd(listar_recebidas(&db, &f(None, None, None, None, Some("AMBEV"))).await.unwrap()), 3);
        assert_eq!(qtd(listar_recebidas(&db, &f(None, None, None, None, Some("xyz"))).await.unwrap()), 0);

        // emitidas com data de setembro (o cadastro é agora)
        gravar_emitida(&db, &RegistroEmitida { referencia: "nfce_a".into(), data_emissao: Some("2026-09-05T10:00:00.000Z".into()), valor_total: Some(30.0), ..Default::default() })
            .await
            .unwrap();
        gravar_emitida(&db, &RegistroEmitida { referencia: "nfe_b".into(), data_emissao: Some("2026-09-20T10:00:00-03:00".into()), valor_total: Some(200.0), ..Default::default() })
            .await
            .unwrap();
        let r = resumo_fiscal_mes(&db, 2026, 9).await.unwrap().unwrap();
        assert_eq!(
            r,
            ResumoFiscalMes {
                month: "2026-09".into(),
                emitidas_count: 2,
                emitidas_total: 230.0,
                nfce_count: 1,
                nfce_total: 30.0,
                nfe_count: 1,
                nfe_total: 200.0,
                recebidas_count: 1,
                recebidas_total: 100.0,
                has_notes: true,
            }
        );
        let hoje = Utc::now();
        let atual = resumo_fiscal_mes(&db, hoje.year(), hoje.month()).await.unwrap().unwrap();
        assert_eq!(atual.emitidas_count, 2); // cadastradas agora
        assert_eq!(resumo_fiscal_mes(&db, 2026, 13).await.unwrap(), None);
        assert!(!resumo_fiscal_mes(&db, 2001, 1).await.unwrap().unwrap().has_notes);
    }

    #[tokio::test]
    async fn sincronizacao_e_busca_pela_chave() {
        let db = Db::open_in_memory().await.unwrap();
        let reg = |r: &str, st: &str| RegistroEmitida { referencia: r.into(), status: Some(st.into()), ..Default::default() };
        gravar_emitida(&db, &reg("nfe_1", "processando")).await.unwrap();
        gravar_emitida(&db, &reg("nfce_2", "processando")).await.unwrap();
        gravar_emitida(&db, &RegistroEmitida { numero: Some("9".into()), ..reg("nfe_3", "autorizado") }).await.unwrap();
        let pend = emitidas_para_sincronizar(&db, TipoNota::Nfe, 15).await.unwrap();
        assert_eq!(pend.iter().map(|n| n.referencia.as_str()).collect::<Vec<_>>(), ["nfe_1"]);

        let atualizou = atualizar_emitida(
            &db,
            &RegistroEmitida { chave: Some(CHAVE_55.into()), numero: Some("1".into()), ..reg("nfe_1", "autorizado") },
        )
        .await
        .unwrap();
        assert!(atualizou);
        assert!(!atualizar_emitida(&db, &reg("nao_existe", "autorizado")).await.unwrap());
        assert!(buscar_emitida(&db, "nao_existe").await.unwrap().is_none());
        let n = buscar_emitida(&db, "nfe_1").await.unwrap().unwrap();
        assert_eq!((n.status.as_str(), n.valor_total), ("autorizado", Some(0.0)));
        assert!(emitidas_para_sincronizar(&db, TipoNota::Nfe, 15).await.unwrap().is_empty());
        assert_eq!(buscar_emitidas_chave_contendo(&db, "000000001").await.unwrap().len(), 1);
        assert!(buscar_emitidas_chave_contendo(&db, "999999999").await.unwrap().is_empty());

        let (emitidas, recebidas) = notas_do_mes(&db, Utc::now().year(), Utc::now().month()).await.unwrap().unwrap();
        assert_eq!((emitidas.len(), recebidas.len()), (3, 0));
        assert!(notas_do_mes(&db, 2026, 0).await.unwrap().is_none());
    }

    #[test]
    fn cofre() {
        let base = std::env::temp_dir().join(format!("barerp-cofre-{}", ids::cuid()));
        assert_eq!(
            caminho_cofre(&base, LadoCofre::Entrada, CHAVE_55, 2026, 9).unwrap(),
            base.join("2026").join("09").join("ENTRADA").join(format!("{CHAVE_55}.xml"))
        );
        assert_eq!(
            caminho_cofre(&base, LadoCofre::Saida, "../../etc/passwd", 2026, 10).unwrap(),
            base.join("2026").join("10").join("SAIDA").join("etcpasswd.xml")
        );
        assert_eq!(caminho_cofre(&base, LadoCofre::Saida, "../", 2026, 10), None);

        let p = arquivar_xml(&base, LadoCofre::Saida, CHAVE_65, "<nfeProc/>").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "<nfeProc/>");
        assert!(p.starts_with(&base) && p.to_string_lossy().contains("SAIDA"));
        assert!(arquivar_xml(&base, LadoCofre::Saida, "", "x").is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
}
