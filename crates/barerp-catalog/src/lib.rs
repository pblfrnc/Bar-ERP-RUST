//! Preenchimento automático do cadastro pelo código de barras (EAN/GTIN).
//! Tradução de `server/src/services/catalogService.ts` (`lookupEanCatalog` e `inferTaxAndClassification`),
//! com uma diferença de propósito: na nota XML o item é o bloco `<det>` que tem o EAN (a regex do Node podia
//! começar no primeiro item da nota e trazer o nome de outro produto).
//!
//! Ordem da busca ("Busca Geral"): produto já cadastrado → notas recebidas (XML) → Open Food Facts → modelo.
//! "Puxar das Notas" e "Puxar Catálogo Global" consultam só a fonte escolhida.

pub mod conversion;
pub mod menu_groups;
pub mod photo;

use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use sqlx::Row;

use barerp_db::{importacao, Db, DbResult};

const OFF_TIMEOUT: Duration = Duration::from_millis(3500);
const USER_AGENT: &str = "BarErpPro/1.0 (bar-erp-pro@local)";

/// Onde procurar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupMode {
    /// Cadastro, notas e catálogo global, nessa ordem
    All,
    /// Só nas notas fiscais recebidas
    Xml,
    /// Só no catálogo global
    Global,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Source {
    LocalProduct,
    NotaFiscalXml,
    OpenFoodFacts,
}

/// Resultado para preencher o formulário (campos `None` não mexem no que já está digitado).
#[derive(Debug, Clone, PartialEq, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LookupResult {
    pub found: bool,
    pub source: Option<Source>,
    pub ean: String,
    pub name: Option<String>,
    pub code: Option<String>,
    pub brand: Option<String>,
    pub supplier: Option<String>,
    pub description: Option<String>,
    pub cost_price: Option<f64>,
    pub suggested_price: Option<f64>,
    pub suggested_category_id: Option<String>,
    /// BAR, KITCHEN ou NONE
    pub kds_station: Option<String>,
    pub ncm: Option<String>,
    pub cfop: Option<String>,
    pub cest: Option<String>,
    pub unit: Option<String>,
    pub message: String,
}

/// Classificação fiscal e de destino deduzida pelo nome.
#[derive(Debug, Clone, PartialEq)]
pub struct TaxInference {
    pub ncm: &'static str,
    pub cest: Option<&'static str>,
    pub cfop: &'static str,
    pub kds_station: &'static str,
    pub category_keywords: &'static [&'static str],
    pub code_prefix: &'static str,
}

struct Rule {
    pattern: &'static str,
    tax: TaxInference,
}

macro_rules! tax {
    ($ncm:expr, $cest:expr, $cfop:expr, $kds:expr, $kw:expr, $prefix:expr) => {
        TaxInference { ncm: $ncm, cest: $cest, cfop: $cfop, kds_station: $kds, category_keywords: $kw, code_prefix: $prefix }
    };
}

/// Na mesma ordem do Node (refrigerante antes de cerveja por causa do "Guaraná Antarctica").
const RULES: &[Rule] = &[
    Rule {
        pattern: r"refrigerante|coca[-\s]?cola|pepsi|guaran[aá]|fanta|sprite|schweppes|soda|kuat|sukita|ituba[ií]na|dolly|gaseosa",
        tax: tax!("22021000", Some("03.007.00"), "5405", "BAR", &["refrigerante", "bebida", "sem alcool"], "REF"),
    },
    Rule {
        pattern: r"cervej|chopp|chope|beer|pilsen|lager|ipa|puro malte|stout|weiss|heineken|brahma|skol|amstel|eisenbahn|corona|budweiser|spaten|stella|colorado|bohemia|antarctica",
        tax: tax!("22030000", Some("03.001.00"), "5405", "BAR", &["cerveja", "chope", "chopp", "bebida"], "CERV"),
    },
    Rule {
        pattern: r"[aá]gua|mineral|crystal|bonafont|minalba|indaia|lindoya|perrier|san pellegrino",
        tax: tax!("22011000", Some("03.002.00"), "5405", "BAR", &["agua", "água", "bebida"], "AGUA"),
    },
    Rule {
        pattern: r"energ[eé]tico|energy drink|red bull|monster|tnt|baly|burn|gatorade|powerade",
        tax: tax!("22029900", Some("03.010.00"), "5405", "BAR", &["energetico", "energético", "bebida"], "ENERG"),
    },
    Rule {
        pattern: r"cacha[cç]a|aguardente|pinga|51|ypio[cç]a|velho barreiro|salinas|seleta|boazinha|sagadiba",
        tax: tax!("22084000", Some("02.010.00"), "5405", "BAR", &["cachaça", "destilado", "dose", "bebida"], "CACH"),
    },
    Rule {
        pattern: r"vodka|smirnoff|absolut|c[iî]roc|ketel one|grey goose|stolichnaya|orloff",
        tax: tax!("22086000", Some("02.012.00"), "5405", "BAR", &["vodka", "destilado", "dose", "bebida"], "VODK"),
    },
    Rule {
        pattern: r"whisky|whiskey|bourbon|red label|black label|jack daniel|chivas|ballantine|white horse|johnnie walker|old parr|grant",
        tax: tax!("22083020", Some("02.008.00"), "5405", "BAR", &["whisky", "destilado", "dose", "bebida"], "WHIS"),
    },
    Rule {
        pattern: r"gin|tanqueray|beefeater|bombay|gordon|hendrick|bulldog",
        tax: tax!("22085000", Some("02.011.00"), "5405", "BAR", &["gin", "destilado", "drinks", "bebida"], "GIN"),
    },
    Rule {
        pattern: r"vinho|espumante|champanhe|prosecco|cabernet|merlot|malbec|chardonnay|sauvignon|rose|tinto|branco",
        tax: tax!("22042100", Some("02.024.00"), "5405", "BAR", &["vinho", "espumante", "bebida"], "VINH"),
    },
    Rule {
        pattern: r"suco|n[eé]ctar|del valle|maguary|camp|ades|prats|tial",
        tax: tax!("20098990", Some("03.012.00"), "5405", "BAR", &["suco", "bebida", "sem alcool"], "SUCO"),
    },
    Rule {
        pattern: r"snack|salgadinho|batata|ruffles|doritos|cheetos|fandangos|lays|pringles|amendoim|castanha|torcida",
        tax: tax!("19059090", Some("17.031.00"), "5102", "BAR", &["petisco", "snack", "porção", "porcao"], "PETI"),
    },
    Rule {
        pattern: r"chocolate|bombom|barra|kitkat|bis|snickers|sonho de valsa|ouro branco|diamante negro|laka",
        tax: tax!("18063110", Some("17.005.00"), "5405", "BAR", &["sobremesa", "doce", "chocolate"], "DOCE"),
    },
    Rule {
        pattern: r"cigarro|tabaco|marlboro|camel|chesterfield|derby|lucky strike|dunhill|rothmans|palha",
        tax: tax!("24022000", Some("04.001.00"), "5405", "BAR", &["tabacaria", "cigarro"], "TABA"),
    },
    Rule {
        pattern: r"hamb[uú]rguer|burger|por[cç][aã]o|carne|frango|batata frita|pastel|pizza|massa|lanche|sandu[ií]che",
        tax: tax!("21069090", None, "5102", "KITCHEN", &["cozinha", "lanche", "porção", "porcao", "prato"], "COZ"),
    },
    // Volume em ml ou litros: bar. `(?-u:\b)` = fronteira ASCII, como o `\b` do JavaScript
    Rule {
        pattern: r"(?i)(?-u:\b)\d+(?:[.,]\d+)?\s*(?:ml|m\.l\.)(?-u:\b)|(?-u:\b)\d+(?:[.,]\d+)?\s*(?:l|lt|litro|litros)(?-u:\b)",
        tax: tax!("22029900", Some("03.010.00"), "5405", "BAR", &["bar", "bebida", "chope", "cerveja"], "BAR"),
    },
];

static COMPILED: LazyLock<Vec<Regex>> =
    LazyLock::new(|| RULES.iter().map(|r| Regex::new(r.pattern).expect("regex do catálogo")).collect());
static BEVERAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"bebida|drink|dose|garrafa|lata|long neck").unwrap());

/// NCM, CEST, CFOP, destino e palavras de categoria pelo nome/marca/categorias (`inferTaxAndClassification`).
pub fn infer_tax(name: &str, brand: Option<&str>, categories: &[&str]) -> TaxInference {
    let combined = format!("{} {} {}", name, brand.unwrap_or(""), categories.join(" ")).to_lowercase();
    for (rule, re) in RULES.iter().zip(COMPILED.iter()) {
        if re.is_match(&combined) {
            return rule.tax.clone();
        }
    }
    if BEVERAGE.is_match(&combined) {
        tax!("22029900", None, "5102", "BAR", &["bar", "bebida"], "BAR")
    } else {
        tax!("21069090", None, "5102", "NONE", &["geral"], "PRD")
    }
}

/// Busca por código de barras. `off_urls` são os modelos do Open Food Facts com `{ean}` (veja [`Catalog::new`]).
pub struct Catalog {
    db: Db,
    http: reqwest::Client,
    off_urls: Vec<String>,
}

impl Catalog {
    pub fn new(db: Db) -> Self {
        Self::with_urls(
            db,
            vec![
                "https://world.openfoodfacts.org/api/v2/product/{ean}.json".into(),
                "https://br.openfoodfacts.org/api/v0/product/{ean}.json".into(),
            ],
        )
    }

    /// Para testes (servidor falso no lugar do Open Food Facts).
    pub fn with_urls(db: Db, off_urls: Vec<String>) -> Self {
        let http = reqwest::Client::builder().user_agent(USER_AGENT).build().expect("cliente HTTP");
        Self { db, http, off_urls }
    }

    pub async fn lookup(&self, raw_ean: &str, mode: LookupMode) -> DbResult<LookupResult> {
        let ean: String = raw_ean.chars().filter(|c| c.is_ascii_digit()).collect();
        if ean.len() < 7 {
            return Ok(LookupResult {
                ean,
                message: "Código de barras inválido. Deve conter pelo menos 7 dígitos.".into(),
                ..Default::default()
            });
        }
        if mode == LookupMode::All {
            if let Some(found) = self.local_product(&ean).await? {
                return Ok(found);
            }
        }
        if matches!(mode, LookupMode::All | LookupMode::Xml) {
            if let Some(found) = self.search_invoices(&ean).await? {
                return Ok(found);
            }
            if mode == LookupMode::Xml {
                return Ok(LookupResult {
                    source: Some(Source::NotaFiscalXml),
                    message: format!("Código {ean} não foi encontrado em nenhuma Nota Fiscal de entrada importada no sistema."),
                    ean,
                    ..Default::default()
                });
            }
        }
        if matches!(mode, LookupMode::All | LookupMode::Global) {
            if let Some(found) = self.search_open_food_facts(&ean).await? {
                return Ok(found);
            }
            if mode == LookupMode::Global {
                return Ok(LookupResult {
                    source: Some(Source::OpenFoodFacts),
                    message: format!("Código {ean} não foi encontrado no Catálogo Global/Nacional GTIN."),
                    ean,
                    ..Default::default()
                });
            }
        }
        Ok(LookupResult {
            code: Some(format!("PRD-{}", last4(&ean))),
            ncm: Some("22029900".into()),
            cfop: Some("5102".into()),
            unit: Some("un".into()),
            kds_station: Some("BAR".into()),
            message: "Produto não localizado nas bases consultadas. Preencha os dados para concluir o cadastro.".into(),
            ean,
            ..Default::default()
        })
    }

    async fn local_product(&self, ean: &str) -> DbResult<Option<LookupResult>> {
        let row = sqlx::query(
            r#"SELECT "name", "code", "ean", "brand", "supplier", "description", "price", "costPrice", "categoryId",
                      "kdsStation", "ncm", "cfop", "cest", "unit"
               FROM "Product" WHERE "ean" = ? LIMIT 1"#,
        )
        .bind(ean)
        .fetch_optional(self.db.pool())
        .await
        .map_err(barerp_db::DbError::from)?;
        Ok(row.map(|r| {
            let price: f64 = r.get("price");
            LookupResult {
                found: true,
                source: Some(Source::LocalProduct),
                name: r.get("name"),
                code: r.get("code"),
                ean: r.get::<Option<String>, _>("ean").unwrap_or_else(|| ean.to_string()),
                brand: r.get("brand"),
                supplier: r.get("supplier"),
                description: r.get("description"),
                cost_price: r.get("costPrice"),
                suggested_price: Some(price),
                suggested_category_id: r.get("categoryId"),
                kds_station: r.get("kdsStation"),
                ncm: r.get("ncm"),
                cfop: r.get("cfop"),
                cest: r.get("cest"),
                unit: Some(r.get::<Option<String>, _>("unit").filter(|u| !u.is_empty()).unwrap_or_else(|| "un".into())),
                message: "Produto já cadastrado no seu banco de dados!".into(),
            }
        }))
    }

    /// Categoria compatível: "Bar" para itens do bar, senão a primeira com uma das palavras, senão a primeira.
    async fn matched_category(&self, tax: &TaxInference) -> DbResult<Option<String>> {
        let cats: Vec<(String, String)> = sqlx::query_as(r#"SELECT "id", "name" FROM "Category" ORDER BY "sortOrder", "name""#)
            .fetch_all(self.db.pool())
            .await
            .map_err(barerp_db::DbError::from)?;
        let bar = if tax.kds_station == "BAR" { cats.iter().find(|(_, n)| n.to_lowercase() == "bar") } else { None };
        let by_word = || cats.iter().find(|(_, n)| tax.category_keywords.iter().any(|k| n.to_lowercase().contains(k)));
        Ok(bar.or_else(by_word).or(cats.first()).map(|(id, _)| id.clone()))
    }

    async fn search_invoices(&self, ean: &str) -> DbResult<Option<LookupResult>> {
        let notes: Vec<(String, Option<String>)> = sqlx::query_as(
            r#"SELECT "xmlContent", "emitente" FROM "NotaRecebida" WHERE "xmlContent" IS NOT NULL ORDER BY "createdAt" DESC LIMIT 100"#,
        )
        .fetch_all(self.db.pool())
        .await
        .map_err(barerp_db::DbError::from)?;
        for (xml, emitente) in notes {
            if !xml.contains(ean) {
                continue;
            }
            let item = det_with_ean(&xml, ean).unwrap_or(&xml);
            let Some(raw_name) = tag(item, "xProd").map(str::trim).filter(|n| !n.is_empty()) else { continue };
            let cost = tag(item, "vUnCom").and_then(|v| v.trim().parse::<f64>().ok());
            let ncm = tag(item, "NCM").map(|v| v.trim().to_string());
            let cest = tag(item, "CEST").map(|v| v.trim().to_string());
            let mut cfop = tag(item, "CFOP").map(|v| v.trim().to_string()).unwrap_or_else(|| "5102".into());
            if cfop.starts_with('1') || cfop.starts_with('2') {
                // CFOP de entrada vira saída: 1403/1405 (ou com CEST) → 5405; 1102 → 5102
                cfop = if cest.is_some() || cfop.contains("403") || cfop.contains("405") { "5405" } else { "5102" }.into();
            }
            let unit = tag(item, "uCom").map(|u| u.trim().to_lowercase()).unwrap_or_else(|| "un".into());
            let supplier = emit_name(&xml).map(|s| s.trim().to_string()).or(emitente);
            let code = tag(item, "cProd").map(|c| c.trim().to_string()).unwrap_or_else(|| format!("NF-{}", last4(ean)));
            let tax = infer_tax(raw_name, None, &[]);
            let category = self.matched_category(&tax).await?;
            let cost_label = cost.map(|c| format!("{c:.2}")).unwrap_or_else(|| "0.00".into());
            return Ok(Some(LookupResult {
                found: true,
                source: Some(Source::NotaFiscalXml),
                name: Some(raw_name.to_string()),
                code: Some(code),
                ean: ean.to_string(),
                message: format!(
                    r#"[Nota Fiscal/SEFAZ] Produto localizado nas notas recebidas do fornecedor "{}". Custo real: R$ {cost_label}."#,
                    supplier.as_deref().unwrap_or("Distribuidora")
                ),
                supplier,
                cost_price: cost,
                suggested_price: cost.map(|c| (c * 2.2 * 100.0).round() / 100.0),
                suggested_category_id: category,
                kds_station: Some(tax.kds_station.into()),
                ncm: Some(ncm.unwrap_or_else(|| tax.ncm.into())),
                cfop: Some(cfop),
                cest: cest.or(tax.cest.map(str::to_string)),
                unit: Some(unit),
                ..Default::default()
            }));
        }
        Ok(None)
    }

    async fn fetch_off(&self, ean: &str) -> Option<serde_json::Value> {
        for template in &self.off_urls {
            let url = template.replace("{ean}", ean);
            let Ok(Ok(resp)) = tokio::time::timeout(OFF_TIMEOUT, self.http.get(&url).send()).await else { continue };
            if !resp.status().is_success() {
                continue;
            }
            let Ok(Ok(json)) = tokio::time::timeout(OFF_TIMEOUT, resp.json::<serde_json::Value>()).await else { continue };
            if json.get("status").and_then(|s| s.as_i64()) == Some(1) {
                if let Some(p) = json.get("product").filter(|p| p.is_object()) {
                    return Some(p.clone());
                }
            }
        }
        None
    }

    async fn search_open_food_facts(&self, ean: &str) -> DbResult<Option<LookupResult>> {
        let Some(p) = self.fetch_off(ean).await else { return Ok(None) };
        let s = |k: &str| p.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let raw_name = ["product_name_pt", "product_name", "product_name_en", "generic_name_pt", "generic_name"]
            .iter()
            .find_map(|k| s(k))
            .unwrap_or_default();
        let brand = s("brands").or_else(|| {
            p.get("brands_tags").and_then(|t| t.get(0)).and_then(|v| v.as_str()).map(str::to_string)
        });
        let tags: Vec<String> = p
            .get("categories_tags")
            .and_then(|t| t.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let categories = s("categories").unwrap_or_default();

        let mut quantity = s("quantity").unwrap_or_default();
        let mut name = raw_name.trim().to_string();
        if !quantity.is_empty() {
            if quantity.chars().all(|c| c.is_ascii_digit()) {
                static DRINK: LazyLock<Regex> = LazyLock::new(|| {
                    Regex::new(r"(?i)cervej|refrig|bebida|água|agua|suco|chopp|chope|heineken|coca|pepsi|guaran").unwrap()
                });
                quantity.push_str(if DRINK.is_match(&name) { "ml" } else { "g" });
            }
            if !name.to_lowercase().contains(&quantity.to_lowercase()) {
                name = format!("{name} {quantity}");
            }
        }
        let name = if name.trim().is_empty() { raw_name.clone() } else { name };
        let mut cats: Vec<&str> = vec![categories.as_str()];
        cats.extend(tags.iter().map(String::as_str));
        let tax = infer_tax(&name, brand.as_deref(), &cats);
        let cest = tax.cest.map(|c| {
            if c.len() == 7 && !c.contains('.') { format!("{}.{}.{}", &c[..2], &c[2..5], &c[5..]) } else { c.to_string() }
        });
        let category = self.matched_category(&tax).await?;
        let code = match &category {
            Some(id) => {
                let mut conn = self.db.pool().acquire().await.map_err(barerp_db::DbError::from)?;
                importacao::proximo_codigo_categoria(&mut conn, id).await?
            }
            None => format!("{}-{}", tax.code_prefix, last4(ean)),
        };
        let first_brand = brand.as_deref().map(|b| b.split(',').next().unwrap_or("").trim().to_string()).filter(|b| !b.is_empty());
        let description = s("generic_name").or_else(|| (!quantity.is_empty()).then(|| format!("Embalagem {quantity}")));
        Ok(Some(LookupResult {
            found: true,
            source: Some(Source::OpenFoodFacts),
            name: Some(name).filter(|n| !n.trim().is_empty()),
            code: Some(code),
            ean: ean.to_string(),
            brand: first_brand.clone(),
            supplier: first_brand,
            description,
            suggested_category_id: category,
            kds_station: Some(tax.kds_station.into()),
            message: format!("[Catálogo Global] Produto localizado! Tributação oficial e NCM {} preenchidos automaticamente.", tax.ncm),
            ncm: Some(tax.ncm.into()),
            cfop: Some(tax.cfop.into()),
            cest,
            unit: Some("un".into()),
            ..Default::default()
        }))
    }
}

fn last4(ean: &str) -> &str {
    &ean[ean.len().saturating_sub(4)..]
}

/// Conteúdo da primeira `<tag>...</tag>` (sem diferenciar maiúsculas na tag).
fn tag<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let open = format!("<{}>", name.to_ascii_lowercase());
    let close = format!("</{}>", name.to_ascii_lowercase());
    let start = lower.find(&open)? + open.len();
    let end = start + lower[start..].find(&close)?;
    Some(&xml[start..end])
}

/// Bloco `<det ...>...</det>` do item com o EAN em `<cEAN>` ou `<cEANTrib>`.
fn det_with_ean<'a>(xml: &'a str, ean: &str) -> Option<&'a str> {
    let lower = xml.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("<det") {
        let start = from + pos;
        // `<det` seguido de espaço ou `>` (não `<detPag>`)
        let next = lower.as_bytes().get(start + 4).copied();
        let end = lower[start..].find("</det>").map(|e| start + e + "</det>".len())?;
        if matches!(next, Some(b' ') | Some(b'>')) {
            let block = &xml[start..end];
            let has = |t: &str| tag(block, t).map(str::trim) == Some(ean);
            if has("cEAN") || has("cEANTrib") {
                return Some(block);
            }
        }
        from = end;
    }
    None
}

/// `<emit>...<xNome>` (razão social do emitente).
fn emit_name(xml: &str) -> Option<&str> {
    let lower = xml.to_ascii_lowercase();
    let start = lower.find("<emit>")?;
    let end = lower[start..].find("</emit>").map(|e| start + e).unwrap_or(xml.len());
    tag(&xml[start..end], "xNome")
}

#[cfg(test)]
mod tests;
