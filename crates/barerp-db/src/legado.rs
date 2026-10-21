//! "Importar do Sistema Antigo" (ImportXmlModal, uso exclusivo do desenvolvedor): lê o XML de exportação de
//! tabelas do programa antigo (ISO-8859-1) e cadastra categorias, fornecedores e produtos sem duplicar.
//! Tradução de `server/src/routes/importXml.ts`, com estas diferenças de propósito:
//! - tudo numa transação só: um erro no meio não deixa metade importada;
//! - erro de leitura do XML vira mensagem para a tela (o Node respondia 500 com o texto da exceção).

use std::collections::HashMap;

use quick_xml::events::Event as XmlEvent;
use quick_xml::Reader;
use serde::Serialize;
use sqlx::SqliteConnection;

use barerp_core::ids;

use crate::events::Event;
use crate::importacao::proximo_codigo_categoria;
use crate::products::is_bar_product;
use crate::{Db, DbError, DbResult, DbTime};

// ---------- XML ----------

/// Elemento do XML com os filhos e o texto (o suficiente para as tabelas do programa antigo).
#[derive(Debug, Default, Clone)]
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    fn all(&self, name: &str) -> Vec<&Node> {
        self.children.iter().filter(|c| c.name == name).collect()
    }

    /// Texto do filho (vazio se não existe).
    fn get(&self, name: &str) -> String {
        self.child(name).map(|c| c.text.trim().to_string()).unwrap_or_default()
    }
}

/// ISO-8859-1 → texto (cada byte é o código do caractere).
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

fn parse(xml: &str) -> Result<Node, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<Node> = vec![Node::default()];
    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(e)) => {
                stack.push(Node { name: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(), ..Default::default() })
            }
            Ok(XmlEvent::Empty(e)) => {
                let node = Node { name: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(), ..Default::default() };
                stack.last_mut().expect("raiz").children.push(node);
            }
            Ok(XmlEvent::Text(t)) => {
                let text = t.unescape().map(|c| c.into_owned()).unwrap_or_else(|_| String::from_utf8_lossy(&t).into_owned());
                stack.last_mut().expect("raiz").text.push_str(&text);
            }
            Ok(XmlEvent::CData(t)) => stack.last_mut().expect("raiz").text.push_str(&String::from_utf8_lossy(&t)),
            Ok(XmlEvent::End(_)) => {
                if stack.len() > 1 {
                    let node = stack.pop().expect("aberto");
                    stack.last_mut().expect("raiz").children.push(node);
                }
            }
            Ok(XmlEvent::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(format!("XML inválido: {e}")),
        }
    }
    Ok(stack.swap_remove(0))
}

// ---------- Regras ----------

/// Cozinha? Tradução de `isKitchenProduct`.
pub fn is_kitchen_product(name: &str, description: Option<&str>) -> bool {
    let text = format!("{} {}", name, description.unwrap_or("")).to_lowercase();
    const WORDS: [&str; 41] = [
        "hambúrguer", "hamburguer", "burger", "lanche", "sanduíche", "sanduiche", "porção", "porçao", "porcão",
        "porcao", "petisco", "batata", "mandioca", "polenta", "carne", "picanha", "frango", "peixe", "camarão",
        "camarao", "pastel", "pizza", "massa", "espeto", "prato", "salada", "almoço", "almoco", "jantar",
        "tira-gosto", "tira gosto", "tiragosto", "sobremesa", "caldo", "sopa", "molho", "pastelão", "pastelao",
        "torta", "sanduíches", "porções",
    ];
    WORDS.iter().any(|w| text.contains(w))
}

fn number(text: &str) -> f64 {
    let t = text.trim().replace(',', ".");
    // `parseFloat`: o número do começo do texto
    let end = t
        .char_indices()
        .find(|(i, c)| !(c.is_ascii_digit() || *c == '.' || (*i == 0 && (*c == '-' || *c == '+'))))
        .map_or(t.len(), |(i, _)| i);
    t[..end].parse::<f64>().ok().filter(|v| v.is_finite()).unwrap_or(0.0)
}

fn digits(text: &str) -> String {
    text.chars().filter(char::is_ascii_digit).collect()
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct CategoriesSummary {
    pub importadas: u32,
    pub ignoradas: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct SuppliersSummary {
    pub importados: u32,
    pub ignorados: u32,
    pub duplicados: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ProductsSummary {
    pub importados: u32,
    pub ignorados: u32,
    pub atualizados: u32,
    pub bar: u32,
    pub cozinha: u32,
}

/// `resumo` da resposta do Node.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct ImportSummary {
    pub categorias: CategoriesSummary,
    pub fornecedores: SuppliersSummary,
    pub produtos: ProductsSummary,
    pub erros: Vec<String>,
}

impl ImportSummary {
    /// Texto de `mensagem` (uma linha por item, como o Node).
    pub fn message(&self) -> String {
        let mut lines = vec![
            format!("✅ {} categorias importadas", self.categorias.importadas),
            format!(
                "✅ {} fornecedores importados ({} já existiam)",
                self.fornecedores.importados, self.fornecedores.duplicados
            ),
        ];
        if self.produtos.importados > 0 {
            lines.push(format!(
                "✅ {} produtos importados ({} no Bar e {} na Cozinha)",
                self.produtos.importados, self.produtos.bar, self.produtos.cozinha
            ));
        }
        if self.produtos.atualizados > 0 {
            lines.push(format!("🔄 {} produtos atualizados", self.produtos.atualizados));
        }
        if !self.erros.is_empty() {
            lines.push(format!("⚠️ {} erros (veja detalhes)", self.erros.len()));
        }
        lines.join("\n")
    }
}

async fn category_by_names(conn: &mut SqliteConnection, names: &[&str]) -> DbResult<Option<String>> {
    for n in names {
        let id: Option<String> = sqlx::query_scalar(r#"SELECT "id" FROM "Category" WHERE "name" = ? LIMIT 1"#)
            .bind(n)
            .fetch_optional(&mut *conn)
            .await?;
        if id.is_some() {
            return Ok(id);
        }
    }
    Ok(None)
}

async fn ensure_category(conn: &mut SqliteConnection, names: &[&str], icon: &str, sort: i64, code_start: i64) -> DbResult<String> {
    if let Some(id) = category_by_names(conn, names).await? {
        return Ok(id);
    }
    let id = ids::cuid();
    sqlx::query(r#"INSERT INTO "Category" ("id", "name", "icon", "sortOrder", "codeStart") VALUES (?, ?, ?, ?, ?)"#)
        .bind(&id)
        .bind(names[0])
        .bind(icon)
        .bind(sort)
        .bind(code_start)
        .execute(&mut *conn)
        .await?;
    Ok(id)
}

/// Importa o XML de tabelas do sistema antigo. Devolve o resumo e o evento de catálogo.
pub async fn import_tables(db: &Db, bytes: &[u8]) -> DbResult<(ImportSummary, Vec<Event>)> {
    let root = parse(&latin1(bytes)).map_err(DbError::rule)?;
    // <Tabelas> (ou <tabelas>) ou o próprio documento
    let tables = root.child("Tabelas").cloned().unwrap_or(root);
    let mut r = ImportSummary::default();
    let now = DbTime::now();
    let mut tx = db.begin_write().await?;

    // 1. Categorias (subcategorias têm prioridade: são as categorias de produtos)
    let cats = match tables.child("Subcategorias") {
        Some(s) => s.all("subcategoria"),
        None => tables.child("Categorias").map(|c| c.all("categoria")).unwrap_or_default(),
    };
    let mut cat_map: HashMap<String, String> = HashMap::new();
    for cat in cats {
        let nome = cat.get("nome").to_uppercase();
        if nome.is_empty() || nome == "PADRAO" || nome == "IMPORTADOS" {
            r.categorias.ignoradas += 1;
            continue;
        }
        let existing = category_by_names(&mut tx, &[&nome]).await?;
        let id = match existing {
            Some(id) => id,
            None => {
                let id = ids::cuid();
                sqlx::query(r#"INSERT INTO "Category" ("id", "name", "icon", "sortOrder") VALUES (?, ?, 'Package', 10)"#)
                    .bind(&id)
                    .bind(&nome)
                    .execute(&mut **tx)
                    .await?;
                r.categorias.importadas += 1;
                id
            }
        };
        cat_map.insert(cat.get("id"), id);
    }
    let cat_bar = ensure_category(&mut tx, &["Bar", "BAR", "bar"], "Beer", 1, 5001).await?;
    let cat_kitchen = ensure_category(&mut tx, &["Cozinha", "COZINHA", "cozinha"], "UtensilsCrossed", 2, 1001).await?;

    // 2. Fornecedores
    let mut supplier_map: HashMap<String, String> = HashMap::new();
    for f in tables.child("Fornecedores").map(|c| c.all("fornecedor")).unwrap_or_default() {
        let nome = f.get("nome").to_uppercase();
        let razao = Some(f.get("razao_social")).filter(|s| !s.is_empty()).unwrap_or_else(|| f.get("nome"));
        if razao.is_empty() || nome == "PADRAO" || nome == "AJUSTE DE ESTOQUE" {
            r.fornecedores.ignorados += 1;
            continue;
        }
        let doc = digits(&f.get("cnpj_cpf"));
        let cnpj = Some(doc).filter(|d| d != "00000000000000" && d.len() >= 11);
        let phone: String = f.get("fone").chars().filter(|c| c.is_ascii_digit() || "()- ".contains(*c)).collect::<String>().trim().to_string();
        let address = [f.get("endereco"), f.get("numero")].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
        let mut existing: Option<String> = None;
        if let Some(c) = &cnpj {
            existing = sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "document" = ?"#).bind(c).fetch_optional(&mut **tx).await?;
        }
        if existing.is_none() {
            existing = sqlx::query_scalar(r#"SELECT "id" FROM "Supplier" WHERE "name" = ? LIMIT 1"#).bind(&razao).fetch_optional(&mut **tx).await?;
        }
        if let Some(id) = existing {
            supplier_map.insert(f.get("id"), id);
            r.fornecedores.duplicados += 1;
            continue;
        }
        let id = ids::cuid();
        sqlx::query(
            r#"INSERT INTO "Supplier" ("id", "name", "tradeName", "document", "ie", "phone", "address", "city", "createdAt", "updatedAt")
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(&razao)
        .bind(Some(nome.clone()).filter(|n| *n != razao.to_uppercase()))
        .bind(&cnpj)
        .bind(Some(f.get("ie")).filter(|s| !s.is_empty()))
        .bind(Some(phone.clone()).filter(|p| digits(p).len() >= 8))
        .bind(Some(address).filter(|a| !a.is_empty()))
        .bind(Some(f.get("cidade_codigocidade")).filter(|s| !s.is_empty()))
        .bind(now)
        .bind(now)
        .execute(&mut **tx)
        .await?;
        supplier_map.insert(f.get("id"), id);
        r.fornecedores.importados += 1;
    }

    // 3. Produtos
    for p in tables.child("Produtos").map(|c| c.all("produto")).unwrap_or_default() {
        if p.get("ativo") != "1" {
            r.produtos.ignorados += 1;
            continue;
        }
        let tipo = Some(p.get("tipo_produto")).filter(|s| !s.is_empty()).unwrap_or_else(|| "PRODUTO".into()).to_uppercase();
        if tipo != "PRODUTO" {
            r.produtos.ignorados += 1;
            continue;
        }
        let nome = Some(p.get("nome")).filter(|s| !s.is_empty()).unwrap_or_else(|| p.get("descricao"));
        if nome.chars().count() < 2 {
            r.produtos.ignorados += 1;
            continue;
        }
        let venda = number(&p.get("preco_venda"));
        let custo = number(&Some(p.get("preco_custo")).filter(|s| !s.is_empty()).unwrap_or_else(|| p.get("preco_compra")));
        let estoque = number(&p.get("saldo")).round() as i64;
        let ean = Some(digits(&p.get("codigo_barras"))).filter(|e| e.len() >= 8);
        let ncm_raw = digits(&Some(p.get("ncmCodigo")).filter(|s| !s.is_empty()).unwrap_or_else(|| p.child("ncm").map(|n| n.get("codigo")).unwrap_or_default()));
        let ncm = Some(ncm_raw).filter(|n| n.len() == 8 && n != "00000000");
        let cest = Some(digits(&p.get("cestCodigo"))).filter(|c| !c.is_empty() && c != "0000000");
        let code = Some(p.get("codigo")).filter(|s| !s.is_empty());
        let descricao = Some(p.get("observacoes")).filter(|s| !s.is_empty());
        let unidade = [p.get("unidade"), p.get("un"), p.get("unid")].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
        let supplier_id = supplier_map.get(&Some(p.get("id_fornecedor")).filter(|s| !s.is_empty()).unwrap_or_else(|| "0".into())).cloned();

        let is_bar = is_bar_product(&nome, descricao.as_deref(), Some(&unidade));
        let is_kitchen = is_kitchen_product(&nome, descricao.as_deref());
        let (category_id, station) = if is_bar {
            (cat_bar.clone(), "BAR")
        } else {
            let xml_cat = Some(p.get("id_subcategoria"))
                .filter(|s| !s.is_empty())
                .or_else(|| Some(p.get("id_categoria")).filter(|s| !s.is_empty()))
                .unwrap_or_else(|| "0".into());
            match cat_map.get(&xml_cat) {
                Some(id) => (id.clone(), if is_kitchen { "KITCHEN" } else { "NONE" }),
                None => (cat_kitchen.clone(), "KITCHEN"),
            }
        };

        // id, preço, custo, categoria e nome da categoria do produto já cadastrado
        type Existing = (String, f64, Option<f64>, String, Option<String>);
        let mut existing: Option<Existing> = None;
        let sel = r#"SELECT p."id", p."price", p."costPrice", p."categoryId", c."name" FROM "Product" p
                     LEFT JOIN "Category" c ON c."id" = p."categoryId""#;
        if let Some(e) = &ean {
            existing = sqlx::query_as(&format!(r#"{sel} WHERE p."ean" = ? LIMIT 1"#)).bind(e).fetch_optional(&mut **tx).await?;
        }
        if existing.is_none() {
            existing = sqlx::query_as(&format!(r#"{sel} WHERE p."name" = ? LIMIT 1"#)).bind(&nome).fetch_optional(&mut **tx).await?;
        }

        if let Some((id, price, cost, current_cat, cat_name)) = existing {
            let old_import = cat_name.unwrap_or_default().to_uppercase().contains("IMPORTAD");
            let move_category = old_import || (is_bar && current_cat != cat_bar);
            sqlx::query(
                r#"UPDATE "Product" SET "price" = ?, "costPrice" = ?, "stock" = ?,
                     "ean" = COALESCE(?, "ean"), "ncm" = COALESCE(?, "ncm"), "cest" = COALESCE(?, "cest"),
                     "supplierId" = COALESCE(?, "supplierId"),
                     "categoryId" = CASE WHEN ? THEN ? ELSE "categoryId" END,
                     "kdsStation" = CASE WHEN ? THEN ? ELSE "kdsStation" END,
                     "updatedAt" = ?
                   WHERE "id" = ?"#,
            )
            .bind(if venda > 0.0 { venda } else { price })
            .bind(if custo > 0.0 { Some(custo) } else { cost })
            .bind(estoque)
            .bind(&ean)
            .bind(&ncm)
            .bind(&cest)
            .bind(&supplier_id)
            .bind(move_category)
            .bind(&category_id)
            .bind(move_category)
            .bind(station)
            .bind(now)
            .bind(&id)
            .execute(&mut **tx)
            .await?;
            r.produtos.atualizados += 1;
        } else {
            let code = match code {
                Some(c) => c,
                None => proximo_codigo_categoria(&mut tx, &category_id).await?,
            };
            sqlx::query(
                r#"INSERT INTO "Product" ("id", "name", "description", "price", "costPrice", "stock", "trackStock", "ean", "ncm",
                                          "cest", "code", "categoryId", "supplierId", "kdsStation", "createdAt", "updatedAt")
                   VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            )
            .bind(ids::cuid())
            .bind(&nome)
            .bind(&descricao)
            .bind(if venda > 0.0 { venda } else { 0.01 })
            .bind(Some(custo).filter(|c| *c > 0.0))
            .bind(estoque)
            .bind(&ean)
            .bind(&ncm)
            .bind(&cest)
            .bind(&code)
            .bind(&category_id)
            .bind(&supplier_id)
            .bind(station)
            .bind(now)
            .bind(now)
            .execute(&mut **tx)
            .await?;
            r.produtos.importados += 1;
            if is_bar {
                r.produtos.bar += 1;
            } else {
                r.produtos.cozinha += 1;
            }
        }
    }

    // A categoria IMPORTADOS antiga sai se ficou vazia
    if let Some(old) = category_by_names(&mut tx, &["IMPORTADOS", "Importados", "importados"]).await? {
        let used: i64 = sqlx::query_scalar(r#"SELECT COUNT(*) FROM "Product" WHERE "categoryId" = ?"#).bind(&old).fetch_one(&mut **tx).await?;
        if used == 0 {
            sqlx::query(r#"DELETE FROM "Category" WHERE "id" = ?"#).bind(&old).execute(&mut **tx).await?;
        }
    }
    tx.commit().await?;
    Ok((r, vec![Event::CatalogUpdated { scope: "products" }]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="ISO-8859-1"?>
<Tabelas>
  <Subcategorias>
    <subcategoria><id>7</id><nome>Porcoes</nome></subcategoria>
    <subcategoria><id>8</id><nome>PADRAO</nome></subcategoria>
  </Subcategorias>
  <Fornecedores>
    <fornecedor><id>3</id><nome>AMBEV</nome><razao_social>AMBEV S.A.</razao_social><cnpj_cpf>07.526.557/0001-00</cnpj_cpf><fone>(91) 3222-1111</fone><endereco>Rua A</endereco><numero>10</numero></fornecedor>
    <fornecedor><id>4</id><nome>PADRAO</nome></fornecedor>
  </Fornecedores>
  <Produtos>
    <produto><ativo>1</ativo><nome>CERVEJA HEINEKEN 600ML</nome><preco_venda>15,00</preco_venda><preco_custo>8,5</preco_custo><saldo>24,0</saldo><codigo_barras>7896045506873</codigo_barras><ncmCodigo>22030000</ncmCodigo><id_fornecedor>3</id_fornecedor></produto>
    <produto><ativo>1</ativo><nome>PORCAO DE FRITAS</nome><preco_venda>25</preco_venda><id_subcategoria>7</id_subcategoria></produto>
    <produto><ativo>0</ativo><nome>ANTIGO</nome></produto>
    <produto><ativo>1</ativo><tipo_produto>SERVICO</tipo_produto><nome>TAXA</nome></produto>
  </Produtos>
</Tabelas>"#;

    #[tokio::test]
    async fn imports_and_updates_without_duplicating() {
        let db = Db::open_in_memory().await.unwrap();
        let (r, events) = import_tables(&db, XML.as_bytes()).await.unwrap();
        assert_eq!(r.categorias, CategoriesSummary { importadas: 1, ignoradas: 1 });
        assert_eq!(r.fornecedores, SuppliersSummary { importados: 1, ignorados: 1, duplicados: 0 });
        assert_eq!((r.produtos.importados, r.produtos.ignorados, r.produtos.bar, r.produtos.cozinha), (2, 2, 1, 1));
        assert_eq!(events.len(), 1);

        let (station, cat, supplier, ncm): (String, String, Option<String>, Option<String>) = sqlx::query_as(
            r#"SELECT p."kdsStation", c."name", p."supplierId", p."ncm" FROM "Product" p JOIN "Category" c ON c."id" = p."categoryId"
               WHERE p."ean" = '7896045506873'"#,
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((station.as_str(), cat.as_str(), ncm.as_deref()), ("BAR", "Bar", Some("22030000")));
        assert!(supplier.is_some());
        let (station, cat): (String, String) = sqlx::query_as(
            r#"SELECT p."kdsStation", c."name" FROM "Product" p JOIN "Category" c ON c."id" = p."categoryId" WHERE p."name" = 'PORCAO DE FRITAS'"#,
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((station.as_str(), cat.as_str()), ("KITCHEN", "PORCOES"));

        // De novo: nada duplica, os produtos são atualizados e o fornecedor já existe
        let (r2, _) = import_tables(&db, XML.as_bytes()).await.unwrap();
        assert_eq!((r2.produtos.importados, r2.produtos.atualizados, r2.fornecedores.duplicados), (0, 2, 1));
        assert!(r2.message().contains("🔄 2 produtos atualizados"));
    }

    #[test]
    fn latin1_and_numbers() {
        assert_eq!(latin1(&[0x50, 0x6F, 0xE7, 0xE3, 0x6F]), "Poção");
        assert_eq!(number("12,50"), 12.5);
        assert_eq!(number("abc"), 0.0);
        assert!(is_kitchen_product("X-Burger da casa", None));
        assert!(!is_kitchen_product("Heineken", None));
    }

    #[tokio::test]
    async fn invalid_xml_is_a_message() {
        let db = Db::open_in_memory().await.unwrap();
        let err = import_tables(&db, b"<Tabelas><Produtos></Tabelas>").await.unwrap_err().to_string();
        assert!(err.starts_with("XML inválido"), "{err}");
    }
}
