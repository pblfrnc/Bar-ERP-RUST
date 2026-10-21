use barerp_print::kitchen::{kitchen_ticket, KitchenItem, KitchenTicket};
use barerp_print::receipt::{
    consumption_receipt, Establishment, ReceiptItem, ReceiptModifier, ReceiptOrder, ReceiptPayment,
};
use barerp_print::{Codepage, EscPos, PaperWidth};
use chrono::NaiveDateTime;

fn dt(s: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S").unwrap()
}

fn establishment() -> Establishment {
    Establishment {
        nome_fantasia: Some("Bar do Zé".into()),
        razao_social: Some("José da Silva Comércio de Bebidas LTDA".into()),
        cnpj: Some("12345678000190".into()),
        endereco: Some("Rua das Flores, 123 - Centro, São João".into()),
    }
}

/// Mesa paga em parte (haver), com taxa de 12%, desconto, adicionais e promoção.
fn order(status: &str) -> ReceiptOrder {
    ReceiptOrder {
        order_number: 42,
        status: status.into(),
        created_at: dt("2026-10-18 19:30:00"),
        closed_at: Some(dt("2026-10-18 22:05:00")),
        table_name: None,
        table_number: Some(7),
        waiter_name: Some("João".into()),
        customer_name: Some("Maria Conceição".into()),
        items: vec![
            ReceiptItem {
                name: "X-Burguer Artesanal com Cheddar e Cebola Caramelizada".into(),
                quantity: 2,
                unit_price: 38.0,
                total_price: 76.0,
                modifiers: vec![
                    ReceiptModifier {
                        name: "Ao ponto".into(),
                        price: 0.0,
                    },
                    ReceiptModifier {
                        name: "Bacon".into(),
                        price: 4.0,
                    },
                ],
                promotion_name: None,
            },
            ReceiptItem {
                name: "Chopp Pilsen 500ml".into(),
                quantity: 6,
                unit_price: 9.9,
                total_price: 59.4,
                modifiers: vec![],
                promotion_name: Some("Happy Hour".into()),
            },
        ],
        subtotal: 135.4,
        is_service_fee_active: true,
        service_fee: 16.25,
        service_fee_rate: 0.12,
        discount: 10.0,
        total: 141.65,
        paid_amount: 100.0,
        payments: vec![
            ReceiptPayment {
                method: "PIX".into(),
                amount: 60.0,
                notes: Some("Pessoa 1/2".into()),
            },
            ReceiptPayment {
                method: "CREDIT_CARD".into(),
                amount: 40.0,
                notes: None,
            },
        ],
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .chunks(16)
        .map(|c| {
            c.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn conferencia_80mm() {
    let mut doc = EscPos::new(PaperWidth::Mm80, Codepage::Cp860);
    consumption_receipt(
        &mut doc,
        &order("OPEN"),
        Some(&establishment()),
        dt("2026-10-18 21:00:00"),
    );
    insta::assert_snapshot!(doc.to_text());
}

#[test]
fn comprovante_58mm() {
    let mut doc = EscPos::new(PaperWidth::Mm58, Codepage::Cp860);
    consumption_receipt(
        &mut doc,
        &order("PAID"),
        Some(&establishment()),
        dt("2026-10-18 22:06:00"),
    );
    let text = doc.to_text();
    assert!(
        text.lines().all(|l| l.chars().count() <= 32),
        "linha passou de 32 colunas"
    );
    insta::assert_snapshot!(text);
}

#[test]
fn comprovante_balcao_sem_estabelecimento_pc850_bytes() {
    let mut o = order("PAID");
    o.table_number = None;
    o.items.truncate(1);
    o.payments.clear();
    o.is_service_fee_active = false;
    let mut doc = EscPos::new(PaperWidth::Mm58, Codepage::Pc850);
    consumption_receipt(&mut doc, &o, None, dt("2026-10-18 22:06:00"));
    insta::assert_snapshot!(doc.to_text());
    insta::assert_snapshot!("comprovante_balcao_bytes", hex(&doc.to_bytes()));
}

fn ticket() -> KitchenTicket {
    KitchenTicket {
        order_number: 42,
        table_name: Some("Varanda 3".into()),
        table_number: Some(3),
        waiter_name: None,
        created_at: dt("2026-10-18 20:15:09"),
        station: Some("Cozinha".into()),
        items: vec![
            KitchenItem {
                name: "Porção de fritas".into(),
                quantity: 1,
                notes: Some("Sem sal · Molho à parte".into()),
            },
            KitchenItem {
                name: "X-Burguer Artesanal com Cheddar e Cebola Caramelizada".into(),
                quantity: 2,
                notes: Some("Ao ponto, sem cebola, pão sem gergelim e maionese verde extra".into()),
            },
        ],
    }
}

#[test]
fn comanda_cozinha_80mm() {
    let mut doc = EscPos::new(PaperWidth::Mm80, Codepage::Cp860);
    assert!(kitchen_ticket(&mut doc, &ticket()));
    insta::assert_snapshot!(doc.to_text());
}

#[test]
fn comanda_cozinha_58mm_bytes() {
    let mut doc = EscPos::new(PaperWidth::Mm58, Codepage::Cp860);
    assert!(kitchen_ticket(&mut doc, &ticket()));
    insta::assert_snapshot!(doc.to_text());
    insta::assert_snapshot!("comanda_cozinha_58mm_hex", hex(&doc.to_bytes()));
}

#[test]
fn comanda_sem_itens_nao_imprime() {
    let mut t = ticket();
    t.items.clear();
    let mut doc = EscPos::new(PaperWidth::Mm80, Codepage::Cp860);
    assert!(!kitchen_ticket(&mut doc, &t));
    assert_eq!(doc.to_bytes(), vec![0x1B, b'@', 0x1B, b't', 3]);
}
