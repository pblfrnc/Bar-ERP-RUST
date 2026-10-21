//! Integração entre o BarERP App e o motor de impressão térmico ESC/POS (`barerp-print`).
//!
//! - [`print_order_receipt`]: Comprovante de conferência de mesa ou recibo de pagamento a partir do `order_id`.
//! - [`print_consumption_receipt`]: Comprovante a partir de um [`OrderDetail`] já carregado.
//! - [`dispatch_print`]: Envia os bytes para a impressora configurada (TCP 9100 ou Spooler Windows).
//! - [`print_test_page`]: Página de teste da tela "Configurar Impressoras".
//! - [`print_kitchen_payload`]: Comanda da cozinha do evento `kds:new_order` (auto-impressão da Retaguarda).

use chrono::Local;

use barerp_core::pricing::ModifierSnapshot;
use barerp_db::notas;
use barerp_db::orders::{self, OrderDetail};
use barerp_db::printer::{self, PrinterSettings};
use barerp_db::Db;
use barerp_print::kitchen::{kitchen_ticket, KitchenItem, KitchenTicket};
use barerp_print::receipt::{
    consumption_receipt, Establishment, ReceiptItem, ReceiptModifier, ReceiptOrder, ReceiptPayment,
};
use barerp_print::{Codepage, EscPos, PaperWidth};

/// Envia bytes ESC/POS para o destino configurado: IP da impressora de rede (porta 9100) ou o nome de uma
/// impressora instalada no sistema (spooler RAW do Windows, CUPS no macOS/Linux).
pub fn dispatch_print(target: &str, job_name: &str, data: &[u8]) -> Result<(), String> {
    use barerp_print::{PrinterSink, SystemPrinter, TcpPrinter};
    let target = target.trim();
    if target.is_empty() {
        return Err("Destino de impressão não configurado.".to_string());
    }
    if is_network_address(target) {
        return TcpPrinter::new(target).send(job_name, data).map_err(|e| e.to_string());
    }
    SystemPrinter::new(target).send(job_name, data).map_err(|e| e.to_string())
}

/// `192.168.0.50`, `192.168.0.50:9100` ou `[::1]:9100`: impressora de rede (porta RAW).
pub fn is_network_address(target: &str) -> bool {
    use std::net::{IpAddr, SocketAddr};
    let t = target.trim();
    t.parse::<IpAddr>().is_ok() || t.parse::<SocketAddr>().is_ok()
}

/// Impressora do caixa: a configurada ou, vazia, a padrão do sistema ("(Padrão do Sistema Operacional)").
pub fn cashier_target(settings: &PrinterSettings) -> Result<String, String> {
    if let Some(p) = settings.cashier_printer.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        return Ok(p.to_string());
    }
    barerp_print::default_printer().ok_or_else(|| {
        "Nenhuma impressora configurada e o sistema não tem impressora padrão. Escolha a impressora em Retaguarda > Configurar Impressoras."
            .to_string()
    })
}

/// Impressora da cozinha: a configurada ou "(Mesma do Caixa / Balcão)".
pub fn kitchen_target(settings: &PrinterSettings) -> Result<String, String> {
    match settings.kitchen_printer.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => Ok(p.to_string()),
        None => cashier_target(settings),
    }
}

fn paper(settings: &PrinterSettings) -> PaperWidth {
    if settings.paper_width <= 58.0 {
        PaperWidth::Mm58
    } else {
        PaperWidth::Mm80
    }
}

/// Envia o trabalho tantas vezes quanto as cópias configuradas.
fn dispatch_copies(target: &str, job_name: &str, data: &[u8], settings: &PrinterSettings) -> Result<(), String> {
    for _ in 0..settings.copies.clamp(1, 5) {
        dispatch_print(target, job_name, data)?;
    }
    Ok(())
}

/// Página de teste com a largura, o corte e o avanço escolhidos (sem gravar as configurações).
pub fn build_test_page(settings: &PrinterSettings) -> Vec<u8> {
    use barerp_print::{Align, QrEcc};
    let mut doc = EscPos::new(paper(settings), Codepage::Pc850);
    doc.align(Align::Center).bold(true).double(true).line("BAR ERP PRO").double(false);
    doc.line("PÁGINA DE TESTE DA IMPRESSORA").bold(false);
    doc.line(&Local::now().format("%d/%m/%Y %H:%M:%S").to_string());
    doc.divider('-').align(Align::Left);
    doc.row("Bobina", &format!("{} mm ({} colunas)", settings.paper_width, doc.columns()));
    doc.row("Corte automático", if settings.auto_cut { "SIM" } else { "NÃO" });
    doc.row("Avanço extra", &format!("{} linhas", settings.extra_feed_lines));
    doc.divider('-');
    doc.row_dotted("1x CERV HEINEKEN 600ML", "R$ 15,00");
    doc.row_dotted("1x PORÇÃO DE FRITAS", "R$ 25,00");
    doc.bold(true).row("VALOR TOTAL:", "R$ 40,00").bold(false);
    doc.divider('-').align(Align::Center);
    doc.line("ÁÉÍÓÚ ÃÕ Ç àèìòù âêô (acentos)");
    let module = ((settings.qr_size / 100.0) * 4.0).round().clamp(2.0, 8.0) as u8;
    doc.qr("https://www.sefaz.pa.gov.br/nfce/consulta", module, QrEcc::M);
    doc.line("QR Code de teste");
    if settings.extra_feed_lines > 0 {
        doc.feed(settings.extra_feed_lines as u8);
    }
    if settings.auto_cut {
        doc.cut(0);
    }
    doc.to_bytes()
}

/// Imprime a página de teste na impressora do caixa.
pub fn print_test_page(settings: &PrinterSettings) -> Result<String, String> {
    let target = cashier_target(settings)?;
    dispatch_print(&target, "Teste BarERP", &build_test_page(settings)).map_err(|e| format!("Falha na impressão: {e}"))?;
    Ok(format!("Página de teste enviada para {target}."))
}

/// Comanda da cozinha a partir do evento `kds:new_order` (mesmos campos que o App.tsx usava).
pub async fn print_kitchen_payload(db: &Db, payload: &serde_json::Value) -> Result<(), String> {
    let items: Vec<KitchenItem> = payload
        .get("items")
        .and_then(|v| v.as_array())
        .map(|list| {
            list.iter()
                .map(|item| KitchenItem {
                    name: item
                        .pointer("/product/name")
                        .or_else(|| item.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("Item")
                        .to_string(),
                    quantity: item.get("quantity").and_then(|v| v.as_i64()).unwrap_or(1),
                    notes: item.get("notes").and_then(|v| v.as_str()).map(str::to_string),
                })
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        return Ok(());
    }
    let settings = printer::get_printer_settings(db).await.unwrap_or_default();
    let target = kitchen_target(&settings)?;
    let ticket = KitchenTicket {
        order_number: payload.get("orderNumber").and_then(|v| v.as_i64()).unwrap_or(0),
        table_name: payload.get("tableName").and_then(|v| v.as_str()).map(str::to_string),
        table_number: payload.get("tableNumber").and_then(|v| v.as_i64()),
        waiter_name: Some(payload.get("waiterName").and_then(|v| v.as_str()).unwrap_or("Garçom").to_string()),
        created_at: Local::now().naive_local(),
        station: Some("COZINHA".to_string()),
        items,
    };
    if let Some(bytes) = build_kitchen_document(&ticket, &settings) {
        dispatch_copies(&target, &format!("Cozinha #{}", ticket.order_number), &bytes, &settings)?;
    }
    Ok(())
}

/// Converte o detalhe da comanda do banco para a estrutura do cupom não-fiscal.
pub fn order_to_receipt(detail: &OrderDetail) -> ReceiptOrder {
    let h = &detail.head;
    let items = detail
        .items
        .iter()
        .map(|item| {
            let mods: Vec<ReceiptModifier> = item
                .modifiers
                .as_deref()
                .and_then(|m| serde_json::from_str::<Vec<ModifierSnapshot>>(m).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|m| ReceiptModifier { name: m.name, price: m.price })
                .collect();

            let name = if item.unit_type.as_deref() == Some("BOX") {
                format!("{} (caixa)", item.product_name)
            } else {
                item.product_name.clone()
            };

            ReceiptItem {
                name,
                quantity: item.quantity,
                unit_price: item.unit_price,
                total_price: item.total_price,
                modifiers: mods,
                promotion_name: item.promotion_name.clone(),
            }
        })
        .collect();

    let payments = detail
        .payments
        .iter()
        .map(|p| ReceiptPayment {
            method: p.method.clone(),
            amount: p.amount,
            notes: p.notes.clone(),
        })
        .collect();

    ReceiptOrder {
        order_number: h.order_number,
        status: h.status.clone(),
        created_at: h.created_at.0.naive_local(),
        closed_at: h.closed_at.as_ref().map(|t| t.0.naive_local()),
        table_name: h.table_name.clone(),
        table_number: h.table_number,
        waiter_name: h.waiter_name.clone(),
        customer_name: h.customer_name.clone(),
        items,
        subtotal: h.subtotal,
        is_service_fee_active: h.is_service_fee_active,
        service_fee: h.service_fee,
        service_fee_rate: h.service_fee_rate,
        discount: h.discount,
        total: h.total,
        paid_amount: h.paid_amount,
        payments,
    }
}

/// Gera os bytes ESC/POS e envia para a impressora do caixa configurada no banco.
pub async fn print_consumption_receipt(db: &Db, detail: &OrderDetail) -> Result<String, String> {
    let settings = printer::get_printer_settings(db)
        .await
        .map_err(|e| format!("Erro ao ler configurações de impressora: {e}"))?;

    let cashier_printer = cashier_target(&settings)?;

    let fiscal = notas::ler_config_fiscal(db).await.ok().flatten();
    let establishment = fiscal.map(|f| Establishment {
        nome_fantasia: f.nome_fantasia,
        razao_social: f.razao_social,
        cnpj: f.cnpj,
        endereco: match (f.logradouro, f.numero, f.bairro, f.municipio, f.uf) {
            (Some(l), Some(n), Some(b), Some(m), Some(uf)) => Some(format!("{l}, {n} - {b}, {m}/{uf}")),
            _ => f.address_info,
        },
    });

    let receipt_order = order_to_receipt(detail);
    let mut doc = EscPos::new(paper(&settings), Codepage::Pc850);
    consumption_receipt(&mut doc, &receipt_order, establishment.as_ref(), Local::now().naive_local());

    if settings.extra_feed_lines > 0 {
        doc.feed(settings.extra_feed_lines as u8);
    }
    if settings.auto_cut {
        doc.cut(0);
    }

    let bytes = doc.to_bytes();
    let job_name = format!("Comanda #{}", receipt_order.order_number);
    dispatch_copies(&cashier_printer, &job_name, &bytes, &settings).map_err(|e| format!("Falha na impressão: {e}"))?;

    Ok(format!("Conferência da Comanda #{} impressa com sucesso!", receipt_order.order_number))
}

/// Imprime o comprovante de conferência/pagamento buscando o pedido pelo ID.
pub async fn print_order_receipt(db: &Db, order_id: &str) -> Result<String, String> {
    let detail = orders::detail(db, order_id)
        .await
        .map_err(|e| format!("Erro ao carregar comanda para impressão: {e}"))?;
    print_consumption_receipt(db, &detail).await
}

/// Gera o documento ESC/POS da comanda da cozinha/bar.
pub fn build_kitchen_document(ticket: &KitchenTicket, settings: &PrinterSettings) -> Option<Vec<u8>> {
    let mut doc = EscPos::new(paper(settings), Codepage::Pc850);
    if kitchen_ticket(&mut doc, ticket) {
        if settings.extra_feed_lines > 0 {
            doc.feed(settings.extra_feed_lines as u8);
        }
        if settings.auto_cut {
            doc.cut(0);
        }
        Some(doc.to_bytes())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDateTime;

    #[test]
    fn network_or_installed_printer() {
        assert!(is_network_address("192.168.0.50"));
        assert!(is_network_address("192.168.0.50:9100"));
        assert!(!is_network_address("EPSON TM-T20 (cópia 1)"));
        assert!(!is_network_address("POS-80C"));
        assert!(!is_network_address("Elgin i9 v1.2"));
    }

    #[test]
    fn test_page_has_cut_when_enabled() {
        let mut settings = PrinterSettings::default();
        let with_cut = build_test_page(&settings);
        settings.auto_cut = false;
        let without_cut = build_test_page(&settings);
        assert!(with_cut.len() > without_cut.len());
    }

    #[test]
    fn build_receipt_test() {
        let order = ReceiptOrder {
            order_number: 42,
            status: "OPEN".to_string(),
            created_at: NaiveDateTime::parse_from_str("2026-10-18 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            closed_at: None,
            table_name: Some("Mesa 5".to_string()),
            table_number: Some(5),
            waiter_name: Some("Carlos".to_string()),
            customer_name: None,
            items: vec![ReceiptItem {
                name: "Chopp Pilsen 500ml".to_string(),
                quantity: 2,
                unit_price: 12.0,
                total_price: 24.0,
                modifiers: vec![],
                promotion_name: None,
            }],
            subtotal: 24.0,
            is_service_fee_active: true,
            service_fee: 2.4,
            service_fee_rate: 0.10,
            discount: 0.0,
            total: 26.4,
            paid_amount: 0.0,
            payments: vec![],
        };
        let settings = PrinterSettings::default();
        let paper = if settings.paper_width <= 58.0 {
            PaperWidth::Mm58
        } else {
            PaperWidth::Mm80
        };
        let mut doc = EscPos::new(paper, Codepage::Pc850);
        consumption_receipt(&mut doc, &order, None, Local::now().naive_local());
        let bytes = doc.to_bytes();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn build_kitchen_ticket_test() {
        let ticket = KitchenTicket {
            order_number: 42,
            table_name: Some("Mesa 5".to_string()),
            table_number: Some(5),
            waiter_name: Some("Carlos".to_string()),
            created_at: NaiveDateTime::parse_from_str("2026-10-18 12:00:00", "%Y-%m-%d %H:%M:%S").unwrap(),
            station: Some("COZINHA".to_string()),
            items: vec![KitchenItem {
                name: "Hambúrguer Artesanal".to_string(),
                quantity: 1,
                notes: Some("SEM CEBOLA".to_string()),
            }],
        };
        let settings = PrinterSettings::default();
        let bytes = build_kitchen_document(&ticket, &settings).unwrap();
        assert!(!bytes.is_empty());
    }
}
