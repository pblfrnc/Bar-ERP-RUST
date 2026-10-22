//! Página "fiscal" (Módulo Fiscal Central do app original). Ver `paridade/fiscal.md` para a lista de funções
//! do original e o estado do port.
//!
//! A tela fala com as mesmas rotas `/api/fiscal` que os celulares usam, mas dentro do processo
//! ([`barerp_server::ServerHandle::fiscal`]), então emissão, cancelamento e notas recebidas seguem uma
//! regra só. Nenhuma chamada sai daqui direto para a Focus.

mod arquivos;
mod danfe;

use barerp_db::printer;
use barerp_fiscal::{ler_danfe_nfce, ratear_desconto};
use barerp_print::{Codepage, EscPos, PaperWidth};
use barerp_server::{Corpo, RespostaFiscal, ServerHandle};
use serde_json::{json, Value};

use chrono::Datelike;
use slint::{ComponentHandle, SharedString, Timer, TimerMode};

use crate::ctx::AppCtx;
use crate::{AppWindow, FiscalStore, FxPainel, FxPainelNota, FxPainelRecebida, FxBackup, FxBip, FxCategoria, FxConfirma, FxItemNfce, FxNcmPedido, FxNota, FxNotaAchada, FxPendente, FxProdutoAchado, FxProdutoBusca, FxRecebida, FxItemImport, FxNfeOk, FxQueda, FxSefaz, FxValidacao, FxVendaOk};

thread_local! {
    /// Atualiza o selo da SEFAZ a cada 5 minutos, como o SefazStatusBadge.
    static TIMER_SEFAZ: Timer = Timer::default();
    /// Notas das listas de reimpressão e cancelamento (o índice da tela aponta para cá).
    static NOTAS_RP: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) };
    static NOTAS_CN: std::cell::RefCell<Vec<Value>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Referência da nota achada na reimpressão / selecionada no cancelamento.
    static ACHADA: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    static CANCELAR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// Chave que a Importação (2º bip) abre já carregada; vazia = upload manual do XML.
    static CHAVE_IMPORTAR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// Emissor de NFC-e: estado da tela (produtos, itens, busca, NCM pendente, última venda).
    static EMISSOR: std::cell::RefCell<Emissor> = std::cell::RefCell::new(Emissor::default());
    /// Conferência do 2º bip (itens, vínculos e preços).
    static IMPORTACAO: std::cell::RefCell<Importacao> = std::cell::RefCell::new(Importacao::default());
    /// Certificado A1 escolhido (nome do arquivo, conteúdo).
    static CERTIFICADO: std::cell::RefCell<Option<(String, Vec<u8>)>> = const { std::cell::RefCell::new(None) };
}

pub fn bind(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let store = ui.global::<FiscalStore>();

    let c = ctx.clone();
    store.on_abrir(move |tela| abrir(&c, &tela));
    let c = ctx.clone();
    // A consulta de NCM (NcmLookupModal) é da área Cardápio: abre a janela dela por cima da tela de produtos
    store.on_abrir_ncm(move || {
        if let Some(ui) = c.ui() {
            ui.set_page(crate::Page::Products);
            ui.global::<crate::CardapioStore>().invoke_open_ncm();
        }
    });

    let c = ctx.clone();
    store.on_sefaz_atualizar(move || atualizar_sefaz(&c));
    atualizar_sefaz(ctx);
    let c = ctx.clone();
    store.on_painel_atualizar(move || carregar_painel(&c));
    carregar_painel(ctx);
    let c = ctx.clone();
    TIMER_SEFAZ.with(|t| {
        t.start(TimerMode::Repeated, std::time::Duration::from_secs(5 * 60), move || {
            atualizar_sefaz(&c);
            carregar_painel(&c);
        })
    });

    let c = ctx.clone();
    store.on_inut_abrir(move |modelo| abrir_inutilizacao(&c, &modelo));
    let c = ctx.clone();
    store.on_inut_enviar(move || enviar_inutilizacao(&c));

    let c = ctx.clone();
    store.on_rp_recarregar(move || carregar_recentes(&c));
    let c = ctx.clone();
    store.on_rp_buscar(move || buscar_reimpressao(&c, None));
    let c = ctx.clone();
    store.on_rp_reimprimir(move |i| {
        let nota = NOTAS_RP.with(|n| n.borrow().get(i as usize).cloned());
        if let (Some(nota), Some(ui)) = (nota, c.ui()) {
            let s = store_de(&ui);
            let nfe = s.get_tela() == "reimprimir-nfe";
            let alvo = Some(texto(&nota, "numero")).filter(|n| !n.is_empty()).or_else(|| nfe.then(|| texto(&nota, "referencia")));
            let Some(alvo) = alvo else { return };
            s.set_rp_busca(alvo.clone().into());
            if nfe && !texto(&nota, "serie").is_empty() {
                s.set_rp_serie(texto(&nota, "serie").into());
            }
            buscar_reimpressao(&c, Some(alvo));
        }
    });
    let c = ctx.clone();
    store.on_rp_imprimir(move || imprimir_achada(&c));
    let c = ctx.clone();
    store.on_rp_ver(move || {
        let referencia = ACHADA.with(|a| a.borrow().clone());
        if let Some(server) = &c.server {
            crate::ctx::open_external(&format!("http://127.0.0.1:{}/api/fiscal/danfe/{}", server.port(), uri_seg(&referencia)));
        }
    });

    let c = ctx.clone();
    store.on_cn_recarregar(move || carregar_cancelaveis(&c));
    let c = ctx.clone();
    store.on_cn_selecionar(move |i| {
        let nota = NOTAS_CN.with(|n| n.borrow().get(i as usize).cloned());
        if let (Some(nota), Some(ui)) = (nota, c.ui()) {
            let s = store_de(&ui);
            let nfe = s.get_tela() == "cancelar-nfe";
            abrir_confirmacao(&s, &nota, nfe);
        }
    });
    let c = ctx.clone();
    store.on_cn_localizar(move || localizar_para_cancelar(&c));
    let c = ctx.clone();
    store.on_cn_confirmar(move || confirmar_cancelamento(&c));

    let c = ctx.clone();
    store.on_cfg_cnpj_editado(move |t| cnpj_editado(&c, &t));
    let c = ctx.clone();
    store.on_cfg_salvar(move || salvar_config(&c));
    let c = ctx.clone();
    store.on_cfg_testar(move || testar_conexao(&c));
    let c = ctx.clone();
    store.on_cfg_diagnosticar(move || diagnosticar(&c));
    let c = ctx.clone();
    store.on_cfg_escolher_cert(move || escolher_certificado(&c));
    let c = ctx.clone();
    store.on_cfg_enviar_cert(move || enviar_certificado(&c));
    store.on_cfg_abrir_focus(|| crate::ctx::open_external("https://app.focusnfe.com.br"));

    let c = ctx.clone();
    store.on_ct_mes_mudou(move |m| mudar_mes(&c, &m));
    let c = ctx.clone();
    store.on_ct_mes_passo(move |passo| {
        let Some(ui) = c.ui() else { return };
        if let Some((a, m)) = ano_mes(&store_de(&ui).get_ct_mes()) {
            let total = a * 12 + (m as i32 - 1) + passo;
            mudar_mes(&c, &format!("{:04}-{:02}", total.div_euclid(12), total.rem_euclid(12) + 1));
        }
    });
    let c = ctx.clone();
    store.on_ct_exportar(move || exportar_mes(&c));
    let c = ctx.clone();
    store.on_ct_bk_atualizar(move || carregar_backups(&c));
    store.on_ct_abrir_url(|url| crate::ctx::open_external(&url));

    let c = ctx.clone();
    store.on_rb_bipar(move || bipar(&c));
    let c = ctx.clone();
    store.on_rb_recarregar(move || carregar_recebidas(&c));
    let c = ctx.clone();
    store.on_rb_buscar_xml(move |chave| buscar_xml_recebida(&c, &chave));
    let c = ctx.clone();
    store.on_pd_filtrar(move || carregar_pendentes(&c));
    let c = ctx.clone();
    store.on_pd_periodo(move |p| periodo_rapido(&c, &p));
    let c = ctx.clone();
    store.on_pd_limpar(move || {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        s.set_pd_inicio(SharedString::new());
        s.set_pd_fim(SharedString::new());
        s.set_pd_busca(SharedString::new());
        carregar_pendentes(&c);
    });
    let c = ctx.clone();
    store.on_pd_sincronizar(move || sincronizar_pendentes(&c));
    let c = ctx.clone();
    store.on_pd_fechar_aviso(move || {
        if let Some(ui) = c.ui() {
            store_de(&ui).set_pd_aviso(SharedString::new());
        }
    });
    let c = ctx.clone();
    store.on_pd_acao(move |acao, chave| pedir_confirmacao(&c, &acao, &chave));
    let c = ctx.clone();
    store.on_pd_confirmar(move |sim| confirmar_acao_pendente(&c, sim));
    let c = ctx.clone();
    store.on_pd_buscar_xml(move |chave| buscar_xml_pendente(&c, &chave));
    let c = ctx.clone();
    store.on_pd_baixar_xml(move |chave| baixar_xml_pendente(&c, &chave));
    let c = ctx.clone();
    store.on_pd_ver_danfe(move |chave| {
        if let Some(server) = &c.server {
            crate::ctx::open_external(&format!("http://127.0.0.1:{}/api/fiscal/danfe/{}", server.port(), so_digitos(&chave)));
        }
    });
    let c = ctx.clone();
    store.on_pd_copiar(move |chave| {
        arquivos::copiar(&chave);
        if let Some(ui) = c.ui() {
            store_de(&ui).set_pd_copiada(chave.clone());
        }
        let c2 = c.clone();
        Timer::single_shot(std::time::Duration::from_millis(2500), move || {
            if let Some(ui) = c2.ui() {
                store_de(&ui).set_pd_copiada(SharedString::new());
            }
        });
    });

    let c = ctx.clone();
    store.on_nf_busca_editada(move |t| {
        if let Some(ui) = c.ui() {
            store_de(&ui).set_nf_lista_aberta(true);
        }
        filtrar_produtos(&c, &t);
    });
    let c = ctx.clone();
    store.on_nf_busca_enter(move || busca_enter(&c));
    let c = ctx.clone();
    store.on_nf_adicionar(move |i| {
        let id = EMISSOR.with(|e| e.borrow().resultados.get(i as usize).cloned());
        if let Some(id) = id {
            adicionar_produto(&c, id);
        }
    });
    let c = ctx.clone();
    store.on_nf_quantidade(move |i, delta| {
        EMISSOR.with(|e| {
            if let Some(item) = e.borrow_mut().itens.get_mut(i as usize) {
                if item.1 + delta >= 1 {
                    item.1 += delta;
                }
            }
        });
        mostrar_itens(&c);
    });
    let c = ctx.clone();
    store.on_nf_remover(move |i| {
        EMISSOR.with(|e| {
            let mut e = e.borrow_mut();
            if (i as usize) < e.itens.len() {
                e.itens.remove(i as usize);
            }
        });
        mostrar_itens(&c);
    });
    let c = ctx.clone();
    store.on_nf_limpar(move || {
        EMISSOR.with(|e| e.borrow_mut().itens.clear());
        mostrar_itens(&c);
    });
    let c = ctx.clone();
    store.on_nf_doc_editado(move |t| {
        let Some(ui) = c.ui() else { return };
        let mascarado = barerp_core::fiscal_doc::format_cpf_cnpj(&t);
        if mascarado != t.as_str() {
            store_de(&ui).set_nf_doc(mascarado.into());
        }
        validar_documento(&c);
    });
    let c = ctx.clone();
    store.on_nf_emitir(move || emitir_nfce_manual(&c));
    let c = ctx.clone();
    store.on_nf_fechar_aviso(move || {
        if let Some(ui) = c.ui() {
            store_de(&ui).set_nf_aviso(SharedString::new());
        }
    });
    let c = ctx.clone();
    store.on_nf_imprimir(move || {
        let referencia = EMISSOR.with(|e| e.borrow().referencia.clone());
        if !referencia.is_empty() {
            imprimir_danfe_nfce(&c, &referencia);
            if let Some(ui) = c.ui() {
                let s = store_de(&ui);
                let mut v = s.get_nf_venda();
                v.impresso = true;
                s.set_nf_venda(v);
            }
        }
    });
    let c = ctx.clone();
    store.on_nf_ver_danfe(move || {
        let referencia = EMISSOR.with(|e| e.borrow().referencia.clone());
        if let (Some(server), false) = (&c.server, referencia.is_empty()) {
            crate::ctx::open_external(&format!("http://127.0.0.1:{}/api/fiscal/danfe/{}", server.port(), uri_seg(&referencia)));
        }
    });
    let c = ctx.clone();
    store.on_nf_copiar_chave(move || {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let chave = if s.get_ne_nota().visivel { s.get_ne_nota().chave } else { s.get_nf_venda().chave };
        arquivos::copiar(&chave);
        s.set_nf_copiada(true);
        let c2 = c.clone();
        Timer::single_shot(std::time::Duration::from_secs(2), move || {
            if let Some(ui) = c2.ui() {
                store_de(&ui).set_nf_copiada(false);
            }
        });
    });
    let c = ctx.clone();
    store.on_nf_enviar_email(move || enviar_email_nfce(&c));
    let c = ctx.clone();
    store.on_nf_nova_venda(move || nova_venda(&c));
    let c = ctx.clone();
    store.on_nf_ncm_editado(move |i, t| {
        let limpo: String = so_digitos(&t).chars().take(8).collect();
        EMISSOR.with(|e| {
            if let Some(v) = e.borrow_mut().ncm_digitado.get_mut(i as usize) {
                *v = limpo;
            }
        });
        mostrar_pedidos_ncm(&c);
    });
    let c = ctx.clone();
    store.on_nf_ncm_cancelar(move || {
        EMISSOR.with(|e| {
            let mut e = e.borrow_mut();
            e.sem_ncm.clear();
            e.ncm_digitado.clear();
        });
        mostrar_pedidos_ncm(&c);
    });
    let c = ctx.clone();
    store.on_nf_ncm_salvar(move || salvar_ncm_e_emitir(&c));

    let c = ctx.clone();
    store.on_ne_doc_editado(move |t| documento_nfe(&c, &t));
    let c = ctx.clone();
    store.on_ne_buscar_cnpj(move || {
        if let Some(ui) = c.ui() {
            let cnpj = so_digitos(&store_de(&ui).get_ne_doc());
            consultar_cnpj_receita(&c, cnpj);
        }
    });
    let c = ctx.clone();
    store.on_ne_cep_editado(move |t| cep_editado(&c, &t));
    let c = ctx.clone();
    store.on_ne_emitir(move || emitir_nfe(&c));
    let c = ctx.clone();
    store.on_ne_imprimir(move || {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let mut n = s.get_ne_nota();
        abrir_pdf_danfe(&c, &n.referencia);
        n.impresso = true;
        s.set_ne_nota(n);
    });
    let c = ctx.clone();
    store.on_ne_nova(move || nova_nfe(&c));

    let c = ctx.clone();
    store.on_importar(move |chave| {
        CHAVE_IMPORTAR.with(|k| *k.borrow_mut() = chave.to_string());
        abrir(&c, "importar");
    });
    ligar_importacao(ctx, &store);
}

fn store_de(ui: &AppWindow) -> FiscalStore<'_> {
    ui.global::<FiscalStore>()
}

/// Troca de tela (o React trocava `activeTab`).
/// "1.284" (número com ponto de milhar).
fn milhar(n: i64) -> String {
    let s = n.abs().to_string();
    let grupos: Vec<&str> = s.as_bytes().rchunks(3).rev().map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    format!("{}{}", if n < 0 { "-" } else { "" }, grupos.join("."))
}

/// Situação da nota emitida para o painel: texto e tom (0 verde, 1 âmbar, 2 vermelho, 3 neutro).
fn situacao_emitida(status: &str) -> (&'static str, i32) {
    match status {
        "autorizado" => ("Autorizada", 0),
        "processando" => ("Processando", 1),
        "erro_autorizacao" => ("Rejeitada", 2),
        "cancelado" => ("Cancelada", 3),
        _ => ("Sem retorno", 1),
    }
}

/// Painel fiscal (tela inicial): notas do dia e do mês, numeração, pendências, últimas notas e notas de fornecedor.
fn carregar_painel(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mut atual = s.get_painel();
    atual.carregando = true;
    s.set_painel(atual);
    let db = ctx.db.clone();
    ctx.spawn(async move { barerp_db::notas::painel_fiscal(&db, chrono::Utc::now()).await }, |ui, r| match r {
        Ok(p) => mostrar_painel(ui, &p),
        Err(e) => {
            tracing::warn!("painel fiscal: {e}");
            let s = store_de(ui);
            let mut painel = s.get_painel();
            painel.carregando = false;
            s.set_painel(painel);
        }
    });
}

fn mostrar_painel(ui: &AppWindow, p: &barerp_db::notas::PainelFiscal) {
    let s = store_de(ui);
    let brl = barerp_core::money::brl;
    let proxima = |n: &barerp_db::notas::Numeracao| {
        let proximo = n.ultimo_emitido.map_or(n.proximo_configurado, |u| (u + 1).max(n.proximo_configurado));
        format!("{} · série {}", milhar(proximo), n.serie)
    };
    s.set_painel(FxPainel {
        carregando: false,
        nfce_hoje: p.nfce_hoje.0 as i32,
        nfce_hoje_valor: brl(p.nfce_hoje.1).into(),
        nfe_mes: p.nfe_mes.0 as i32,
        nfe_mes_valor: brl(p.nfe_mes.1).into(),
        nfce_numeracao: proxima(&p.numeracao_nfce).into(),
        nfe_numeracao: proxima(&p.numeracao_nfe).into(),
        producao: p.ambiente == "producao",
        rejeitadas: p.rejeitadas as i32,
        processando: p.processando as i32,
        cancelaveis_nfce: p.cancelaveis_nfce as i32,
        cancelaveis_nfe: p.cancelaveis_nfe as i32,
        recebidas_pendentes: p.recebidas_pendentes as i32,
    });
    let notas: Vec<FxPainelNota> = p
        .recentes
        .iter()
        .map(|n| {
            let (situacao, tom) = situacao_emitida(&n.status);
            let modelo = if n.tipo() == barerp_db::notas::TipoNota::Nfce { "NFC-e" } else { "NF-e" };
            let numero = n.numero.as_deref().and_then(|x| x.trim().parse::<i64>().ok()).map(milhar).unwrap_or_else(|| "—".into());
            FxPainelNota {
                modelo: modelo.into(),
                numero: numero.into(),
                quando: n.created_at.0.with_timezone(&chrono::Local).format("%d/%m %H:%M").to_string().into(),
                valor: n.valor_total.map(brl).unwrap_or_default().into(),
                situacao: situacao.into(),
                tom,
            }
        })
        .collect();
    s.set_painel_notas(std::rc::Rc::new(slint::VecModel::from(notas)).into());
    let recebidas: Vec<FxPainelRecebida> = p
        .recebidas
        .iter()
        .map(|r| {
            let (situacao, tom) = if r.has_xml { ("XML baixado", 0) } else { ("XML pendente", 1) };
            let numero = if r.numero.is_empty() { String::new() } else { format!("NF {} · ", r.numero) };
            FxPainelRecebida {
                emitente: if r.emitente.is_empty() { "Fornecedor sem nome".into() } else { r.emitente.clone().into() },
                detalhe: format!("{numero}{}", brl(r.valor_total)).into(),
                situacao: situacao.into(),
                tom,
            }
        })
        .collect();
    s.set_painel_recebidas(std::rc::Rc::new(slint::VecModel::from(recebidas)).into());
}

/// Para os prints das telas (`examples/fiscal_shots.rs`): painel com os números dados, sem servidor.
pub mod preview {
    use super::*;

    pub fn painel(ui: &AppWindow, p: &barerp_db::notas::PainelFiscal, tela: &str) {
        mostrar_painel(ui, p);
        let s = store_de(ui);
        s.set_tela(tela.into());
        s.set_sefaz(FxSefaz { online: true, rotulo: "SEFAZ PA Online".into(), latencia: "180 ms".into(), ..Default::default() });
    }
}

fn abrir(ctx: &AppCtx, tela: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_tela(tela.into());
    match tela {
        "hub" => {
            atualizar_sefaz(ctx);
            carregar_painel(ctx);
        }
        "reimprimir-nfce" | "reimprimir-nfe" => {
            s.set_rp_busca(SharedString::new());
            s.set_rp_serie(SharedString::new());
            s.set_rp_erro(SharedString::new());
            s.set_rp_achada(FxNotaAchada::default());
            carregar_recentes(ctx);
        }
        "cancelar-nfce" | "cancelar-nfe" => {
            s.set_cn_resultado(SharedString::new());
            s.set_cn_erro(SharedString::new());
            s.set_cn_busca(SharedString::new());
            s.set_cn_serie(SharedString::new());
            s.set_cn_confirmando(false);
            carregar_cancelaveis(ctx);
        }
        "config" => carregar_config(ctx),
        "nfce" | "nfe" => {
            EMISSOR.with(|e| *e.borrow_mut() = Emissor::default());
            limpar_destinatario(&s);
            s.set_ne_nota(FxNfeOk::default());
            s.set_nf_busca(SharedString::new());
            s.set_nf_doc(SharedString::new());
            s.set_nf_nome(SharedString::new());
            s.set_nf_pagamento("PIX".into());
            s.set_nf_aviso(SharedString::new());
            s.set_nf_venda(FxVendaOk::default());
            s.set_nf_lista_aberta(false);
            mostrar_itens(ctx);
            mostrar_pedidos_ncm(ctx);
            validar_documento(ctx);
            let db = ctx.db.clone();
            ctx.spawn(async move { barerp_db::importacao::produtos_fiscais(&db).await }, |_, r| {
                if let Ok(p) = r {
                    EMISSOR.with(|e| e.borrow_mut().produtos = p);
                }
            });
        }
        "importar" => abrir_importacao(ctx),
        "receber" => {
            s.set_rb_chave(SharedString::new());
            s.set_rb_resultado(FxBip::default());
            s.set_rb_aviso(SharedString::new());
            carregar_recebidas(ctx);
        }
        "pendentes" => {
            s.set_pd_aviso(SharedString::new());
            s.set_pd_confirma(FxConfirma::default());
            carregar_pendentes(ctx);
        }
        "contador" => {
            s.set_ct_mensagem(SharedString::new());
            let hoje = chrono::Local::now();
            mudar_mes(ctx, &hoje.format("%Y-%m").to_string());
            carregar_backups(ctx);
        }
        _ => {}
    }
}

fn texto(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

// ============================================================
// Selo da SEFAZ
// ============================================================

/// Monta o selo a partir da resposta de `/sefaz-status` (`None` = sem resposta do servidor).
fn selo_sefaz(d: Option<&Value>) -> FxSefaz {
    let Some(d) = d else {
        return FxSefaz {
            carregando: false,
            online: false,
            desconectado: true,
            rotulo: "Servidor ERP Desconectado".into(),
            autorizador: "SEFAZ (PA)".into(),
            motivo: "Sem conexão com servidor".into(),
            dica: "Este PC não consegue se comunicar com o computador principal. Verifique se o IP do servidor nas configurações do app está correto.".into(),
            ..Default::default()
        };
    };
    let online = d.get("online").and_then(Value::as_bool).unwrap_or(false);
    let uf = Some(texto(d, "uf")).filter(|u| !u.is_empty()).unwrap_or_else(|| "PA".into());
    let motivo = Some(texto(d, "motivoStatus"))
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| if online { "Serviço em Operação".into() } else { "Serviço Indisponível".into() });
    let latencia = texto(d, "tempoMedio");
    FxSefaz {
        carregando: false,
        online,
        desconectado: false,
        rotulo: if online { format!("SEFAZ Online ({uf})") } else { format!("SEFAZ ({uf}) Fora do Ar") }.into(),
        autorizador: Some(texto(d, "autorizador")).filter(|a| !a.is_empty()).unwrap_or_else(|| format!("SEFAZ ({uf})")).into(),
        motivo: motivo.into(),
        codigo: texto(d, "codigoStatus").into(),
        latencia: if latencia.is_empty() { SharedString::new() } else { format!("{latencia}s").into() },
        detalhe: texto(d, "detalheToken").into(),
        dica: if online {
            "Emissões fiscais de NFC-e autorizando normalmente."
        } else {
            "Se a SEFAZ estiver instável, a contingência offline entra em ação automaticamente."
        }
        .into(),
    }
}

fn atualizar_sefaz(ctx: &AppCtx) {
    let server = ctx.server.clone();
    ctx.spawn(
        async move {
            let r = chamar(server, "GET", "/sefaz-status", Corpo::Nada).await;
            if r.status == 503 { None } else { Some(r.json()) }
        },
        |ui, r| store_de(ui).set_sefaz(selo_sefaz(r.as_ref())),
    );
}

// ============================================================
// Formatos da tela
// ============================================================

/// "R$ 1.234,56"
fn reais(v: f64) -> String {
    format!("R$ {}", barerp_print::format::money(v))
}

fn numero(v: &Value, k: &str) -> f64 {
    match v.get(k) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// Data ISO do servidor no horário do computador: "18/10/2026, 14:05:09" (`toLocaleString('pt-BR')`).
fn data_local(iso: &str, so_hora: bool) -> String {
    match chrono::DateTime::parse_from_rfc3339(iso) {
        Ok(d) => {
            let d = d.with_timezone(&chrono::Local);
            if so_hora { d.format("%H:%M:%S").to_string() } else { d.format("%d/%m/%Y, %H:%M:%S").to_string() }
        }
        Err(_) => iso.to_string(),
    }
}

// ============================================================
// Inutilização de numeração
// ============================================================

fn abrir_inutilizacao(ctx: &AppCtx, modelo: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_inut_modelo(modelo.into());
    s.set_inut_serie("1".into());
    s.set_inut_inicial(SharedString::new());
    s.set_inut_final(SharedString::new());
    s.set_inut_justificativa(SharedString::new());
    s.set_inut_resultado(SharedString::new());
    s.set_inut_enviando(false);
    // série padrão da configuração fiscal
    let (server, nfe) = (ctx.server.clone(), modelo == "nfe");
    ctx.spawn(
        async move { chamar(server, "GET", "/settings", Corpo::Nada).await.json() },
        move |ui, d| {
            let serie = texto(&d, if nfe { "serieNfe" } else { "serieNfce" });
            if !serie.is_empty() {
                store_de(ui).set_inut_serie(serie.into());
            }
        },
    );
}

fn enviar_inutilizacao(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let nfe = s.get_inut_modelo() == "nfe";
    let serie = s.get_inut_serie().trim().to_string();
    let (ini, fim) = (s.get_inut_inicial().trim().parse::<i64>(), s.get_inut_final().trim().parse::<i64>());
    let justificativa = s.get_inut_justificativa().trim().to_string();
    let erro = |m: &str| {
        s.set_inut_resultado_ok(false);
        s.set_inut_resultado(m.into());
    };
    let (Ok(ini), Ok(fim)) = (ini, fim) else {
        return erro("Preencha a série e o intervalo de números.");
    };
    if serie.is_empty() {
        return erro("Preencha a série e o intervalo de números.");
    }
    if fim < ini {
        return erro("O número final não pode ser menor que o número inicial.");
    }
    if justificativa.chars().count() < 15 {
        return erro("A justificativa deve ter no mínimo 15 caracteres (Exigência legal da SEFAZ).");
    }
    s.set_inut_enviando(true);
    s.set_inut_resultado(SharedString::new());
    let caminho = if nfe { "/inutilizar-numeracao" } else { "/inutilizar-numeracao-nfce" };
    let corpo = json!({ "serie": serie, "numeroInicial": ini, "numeroFinal": fim, "justificativa": justificativa });
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "POST", caminho, Corpo::Json(corpo)).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_inut_enviando(false);
        s.set_inut_resultado_ok(r.ok());
        let msg = if r.ok() {
            Some(texto(&r.json(), "mensagem")).filter(|m| !m.is_empty()).unwrap_or_else(|| {
                if nfe { "Inutilização de NF-e homologada com sucesso!" } else { "Inutilização de NFC-e homologada com sucesso!" }.into()
            })
        } else {
            Some(r.erro()).filter(|m| !m.is_empty()).unwrap_or_else(|| "Erro ao homologar na SEFAZ.".into())
        };
        s.set_inut_resultado(msg.into());
    });
}

/// Chama uma rota fiscal (caminho sem `/api/fiscal`). Sem servidor (testes), responde 503.
async fn chamar(server: Option<ServerHandle>, metodo: &str, caminho: &str, corpo: Corpo) -> RespostaFiscal {
    match server {
        Some(s) => s.fiscal(metodo, caminho, corpo).await,
        None => RespostaFiscal {
            status: 503,
            content_type: "application/json".into(),
            bytes: json!({ "error": "Servidor fiscal indisponível." }).to_string().into_bytes(),
        },
    }
}

// ============================================================
// Reimpressão (NfceReprintView / NfeReprintView)
// ============================================================

fn nota_da_lista(n: &Value) -> FxNota {
    let status = Some(texto(n, "status")).filter(|s| !s.is_empty()).unwrap_or_else(|| "autorizado".into());
    FxNota {
        referencia: texto(n, "referencia").into(),
        numero: texto(n, "numero").into(),
        serie: texto(n, "serie").into(),
        cancelada: status == "cancelado",
        status: status.into(),
        data: data_local(&texto(n, "createdAt"), false).into(),
        valor: if n.get("valorTotal").is_some_and(|v| !v.is_null()) { reais(numero(n, "valorTotal")).into() } else { SharedString::new() },
        chave: texto(n, "chave").into(),
        prazo: SharedString::new(),
        urgencia: 0,
    }
}

fn carregar_recentes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let tipo = if s.get_tela() == "reimprimir-nfe" { "nfe" } else { "nfce" };
    s.set_rp_carregando(true);
    let server = ctx.server.clone();
    ctx.spawn(
        async move { chamar(server, "GET", &format!("/recent-notes?tipo={tipo}"), Corpo::Nada).await.json() },
        |ui, d| {
            let lista = d.as_array().cloned().unwrap_or_default();
            let s = store_de(ui);
            s.set_rp_notas(slint::ModelRc::new(slint::VecModel::from(lista.iter().map(nota_da_lista).collect::<Vec<_>>())));
            s.set_rp_carregando(false);
            NOTAS_RP.with(|n| *n.borrow_mut() = lista);
        },
    );
}

fn buscar_reimpressao(ctx: &AppCtx, alvo: Option<String>) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let nfe = s.get_tela() == "reimprimir-nfe";
    let busca = alvo.unwrap_or_else(|| s.get_rp_busca().trim().to_string());
    let serie = s.get_rp_serie().trim().to_string();
    if busca.is_empty() {
        s.set_rp_erro(
            if nfe { "Digite o número, referência ou chave da NF-e a ser reimpressa." } else { "Digite o número da nota fiscal a ser reimpressa." }
                .into(),
        );
        return;
    }
    s.set_rp_erro(SharedString::new());
    s.set_rp_buscando(true);
    s.set_rp_achada(FxNotaAchada::default());
    let caminho = if nfe {
        let q = if serie.is_empty() { String::new() } else { format!("?serie={}", uri_seg(&serie)) };
        format!("/nfe/reprint/{}{q}", uri_seg(&busca))
    } else {
        format!("/reprint-by-number/{}", uri_seg(&busca))
    };
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", &caminho, Corpo::Nada).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_rp_buscando(false);
        let d = r.json();
        if !r.ok() || d.get("success").and_then(Value::as_bool) != Some(true) {
            s.set_rp_erro(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Nota fiscal não localizada.".into()).into());
            return;
        }
        let nota = d.get("nota").cloned().unwrap_or(Value::Null);
        ACHADA.with(|a| *a.borrow_mut() = texto(&nota, "referencia"));
        s.set_rp_achada(FxNotaAchada {
            visivel: true,
            referencia: texto(&nota, "referencia").into(),
            numero: Some(texto(&nota, "numero")).filter(|n| !n.is_empty()).unwrap_or(busca).into(),
            serie: Some(texto(&nota, "serie")).filter(|n| !n.is_empty()).unwrap_or_else(|| "1".into()).into(),
            status: Some(texto(&d, "status")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Autorizado".into()).into(),
            chave: texto(&d, "chaveAcesso").into(),
            valor: if nota.get("valorTotal").is_some_and(|v| !v.is_null()) { reais(numero(&nota, "valorTotal")).into() } else { SharedString::new() },
        });
    });
}

/// NFC-e: cupom na térmica (ESC/POS). NF-e: PDF A4 da Focus aberto no visualizador do sistema.
fn imprimir_achada(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let referencia = ACHADA.with(|a| a.borrow().clone());
    if referencia.is_empty() {
        return;
    }
    if store_de(&ui).get_tela() != "reimprimir-nfe" {
        imprimir_danfe_nfce(ctx, &referencia);
        ctx.status("Enviando o cupom para a impressora térmica...", false);
        return;
    }
    abrir_pdf_danfe(ctx, &referencia);
}

/// Baixa o DANFE (PDF da Focus) e abre no visualizador de PDF do sistema, de onde se imprime em A4.
fn abrir_pdf_danfe(ctx: &AppCtx, referencia: &str) {
    let (server, referencia) = (ctx.server.clone(), referencia.to_string());
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let r = chamar(server, "GET", &format!("/danfe/{}", uri_seg(&referencia)), Corpo::Nada).await;
            if !r.ok() {
                return Err(r.erro());
            }
            let ext = if r.content_type.contains("pdf") { "pdf" } else { "html" };
            let nome: String = referencia.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
            let caminho = std::env::temp_dir().join(format!("DANFE_{nome}.{ext}"));
            std::fs::write(&caminho, &r.bytes).map_err(|e| e.to_string())?;
            Ok(caminho)
        },
        move |_, r| match r {
            Ok(caminho) => {
                crate::ctx::open_external(&caminho.to_string_lossy());
                c.status("DANFE aberto no visualizador do sistema: use Imprimir (A4).", false);
            }
            Err(e) => c.status(format!("Não foi possível abrir o DANFE: {e}"), true),
        },
    );
}

// ============================================================
// Cancelamento (NfceCancelView / NfeCancelView)
// ============================================================

fn prazo_nfe(horas: i64, minutos: i64) -> String {
    if horas > 0 { format!("{horas}h {minutos}min restantes") } else { format!("{minutos} minutos restantes") }
}

fn nota_cancelavel(n: &Value, nfe: bool) -> FxNota {
    let mut nota = nota_da_lista(n);
    if nfe {
        let (h, m) = (numero(n, "hoursRemaining") as i64, numero(n, "minutesInHour") as i64);
        nota.prazo = prazo_nfe(h, m).into();
        nota.urgencia = if h == 0 && m <= 60 { 2 } else if h <= 6 { 1 } else { 0 };
        nota.data = data_local(&texto(n, "createdAt"), false).into();
    } else {
        nota.prazo = format!("Restam ~{} min", numero(n, "minutesRemaining") as i64).into();
        nota.urgencia = 2;
        nota.data = data_local(&texto(n, "createdAt"), true).into();
    }
    nota.valor = reais(numero(n, "valorTotal")).into();
    nota
}

fn carregar_cancelaveis(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let nfe = s.get_tela() == "cancelar-nfe";
    s.set_cn_carregando(true);
    let server = ctx.server.clone();
    let caminho = if nfe { "/nfe/cancelable-notes" } else { "/cancelable-notes" };
    ctx.spawn(async move { chamar(server, "GET", caminho, Corpo::Nada).await.json() }, move |ui, d| {
        let lista = d.as_array().cloned().unwrap_or_default();
        let s = store_de(ui);
        s.set_cn_notas(slint::ModelRc::new(slint::VecModel::from(
            lista.iter().map(|n| nota_cancelavel(n, nfe)).collect::<Vec<_>>(),
        )));
        s.set_cn_carregando(false);
        NOTAS_CN.with(|n| *n.borrow_mut() = lista);
    });
}

fn abrir_confirmacao(s: &FiscalStore<'_>, nota: &Value, nfe: bool) {
    let mut n = nota_cancelavel(nota, nfe);
    if !nfe {
        n.prazo = format!("~{} minutos", numero(nota, "minutesRemaining") as i64).into();
    }
    CANCELAR.with(|c| *c.borrow_mut() = texto(nota, "referencia"));
    s.set_cn_nota(n);
    s.set_cn_justificativa(SharedString::new());
    s.set_cn_resultado(SharedString::new());
    s.set_cn_confirmando(true);
}

/// NF-e fora da lista automática: localiza pela reimpressão e abre o cancelamento.
fn localizar_para_cancelar(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let busca = s.get_cn_busca().trim().to_string();
    let serie = s.get_cn_serie().trim().to_string();
    if busca.is_empty() {
        s.set_cn_erro("Digite o número, chave ou referência da NF-e a ser cancelada.".into());
        return;
    }
    s.set_cn_erro(SharedString::new());
    s.set_cn_buscando(true);
    let q = if serie.is_empty() { String::new() } else { format!("?serie={}", uri_seg(&serie)) };
    let caminho = format!("/nfe/reprint/{}{q}", uri_seg(&busca));
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", &caminho, Corpo::Nada).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_cn_buscando(false);
        let d = r.json();
        if !r.ok() {
            s.set_cn_erro(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Nota não encontrada.".into()).into());
            return;
        }
        let nota = d.get("nota").cloned().unwrap_or(Value::Null);
        if texto(&d, "status") == "cancelado" || texto(&nota, "status") == "cancelado" {
            s.set_cn_erro("Esta NF-e já consta como CANCELADA na SEFAZ.".into());
            return;
        }
        let mut manual = nota.clone();
        if let Some(o) = manual.as_object_mut() {
            o.entry("referencia").or_insert(json!(busca));
            o.entry("numero").or_insert(json!(busca));
            if texto(&nota, "serie").is_empty() {
                o.insert("serie".into(), json!(if serie.is_empty() { "1".to_string() } else { serie.clone() }));
            }
            o.insert("hoursRemaining".into(), json!(24));
            o.insert("minutesInHour".into(), json!(0));
        }
        abrir_confirmacao(&s, &manual, true);
    });
}

fn confirmar_cancelamento(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let nfe = s.get_tela() == "cancelar-nfe";
    let justificativa = s.get_cn_justificativa().trim().to_string();
    if justificativa.chars().count() < 15 {
        s.set_cn_resultado_ok(false);
        s.set_cn_resultado("A justificativa deve ter no mínimo 15 caracteres (Regra da SEFAZ).".into());
        return;
    }
    let referencia = CANCELAR.with(|c| c.borrow().clone());
    let numero_nota = s.get_cn_nota().numero.to_string();
    s.set_cn_cancelando(true);
    s.set_cn_resultado(SharedString::new());
    let caminho = if nfe { "/cancel-nfe" } else { "/cancel-nfce" };
    let corpo = json!({ "referencia": referencia, "justificativa": justificativa });
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", caminho, Corpo::Json(corpo)).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_cn_cancelando(false);
        s.set_cn_resultado_ok(r.ok());
        if r.ok() {
            let modelo = if nfe { "NF-e" } else { "NFC-e" };
            s.set_cn_resultado(format!("{modelo} nº {numero_nota} cancelada com sucesso na SEFAZ!").into());
            s.set_cn_confirmando(false);
            s.set_cn_justificativa(SharedString::new());
            carregar_cancelaveis(&c);
        } else {
            s.set_cn_resultado(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao cancelar nota na SEFAZ.".into()).into());
        }
    });
}

// ============================================================
// Configurações Fiscais (FiscalSettingsView)
// ============================================================

fn so_digitos(s: &str) -> String {
    s.chars().filter(char::is_ascii_digit).collect()
}

/// -1 incompleto, 0 inválido, 1 válido
fn estado_cnpj(cnpj: &str) -> i32 {
    let d = so_digitos(cnpj);
    if d.len() < 14 {
        -1
    } else if barerp_core::fiscal_doc::validate_cnpj(&d) {
        1
    } else {
        0
    }
}

fn mascara_cep(v: &str) -> String {
    let d: String = so_digitos(v).chars().take(8).collect();
    if d.len() > 5 { format!("{}-{}", &d[..5], &d[5..]) } else { d }
}

fn carregar_config(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_cfg_carregando(true);
    s.set_cfg_mensagem(SharedString::new());
    s.set_cfg_validacao(FxValidacao::default());
    s.set_cfg_diagnostico(SharedString::new());
    s.set_cfg_cert_resultado(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", "/settings", Corpo::Nada).await.json() }, |ui, d| {
        let s = store_de(ui);
        let t = |k: &str| SharedString::from(texto(&d, k));
        s.set_cfg_token(t("apiToken"));
        s.set_cfg_razao(t("razaoSocial"));
        s.set_cfg_fantasia(t("nomeFantasia"));
        s.set_cfg_cnpj(t("cnpj"));
        s.set_cfg_ie(t("ie"));
        s.set_cfg_crt(t("crt"));
        s.set_cfg_csc_id(t("cscId"));
        s.set_cfg_csc(t("cscSecret"));
        let amb = texto(&d, "environment");
        s.set_cfg_ambiente(if amb.is_empty() { "homologacao".into() } else { amb.into() });
        let ou = |k: &str, padrao: &str| SharedString::from(Some(texto(&d, k)).filter(|v| !v.is_empty()).unwrap_or_else(|| padrao.into()));
        s.set_cfg_serie_nfe(ou("serieNfe", "1"));
        s.set_cfg_proximo_nfe(ou("proximoNumeroNfe", "1"));
        s.set_cfg_serie_nfce(ou("serieNfce", "1"));
        s.set_cfg_proximo_nfce(ou("proximoNumeroNfce", "1"));
        s.set_cfg_cep(t("cep"));
        s.set_cfg_logradouro(t("logradouro"));
        s.set_cfg_numero(t("numero"));
        s.set_cfg_bairro(t("bairro"));
        s.set_cfg_municipio(t("municipio"));
        s.set_cfg_uf(t("uf"));
        s.set_cfg_cnpj_valido(if texto(&d, "cnpj").is_empty() { -1 } else { estado_cnpj(&texto(&d, "cnpj")) });
        s.set_cfg_carregando(false);
    });
}

/// Máscara, validação dos dígitos e, com CNPJ válido, preenchimento pela Receita (BrasilAPI).
fn cnpj_editado(ctx: &AppCtx, texto_digitado: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mascarado = barerp_core::fiscal_doc::format_cpf_cnpj(&so_digitos(texto_digitado).chars().take(14).collect::<String>());
    // CNPJ tem 14 dígitos: a máscara de 11 (CPF) só aparece enquanto digita
    let mascarado = if so_digitos(&mascarado).len() == 14 { mascarado } else { texto_digitado.to_string() };
    if mascarado != texto_digitado {
        s.set_cfg_cnpj(mascarado.clone().into());
    }
    let estado = estado_cnpj(&mascarado);
    s.set_cfg_cnpj_valido(estado);
    if estado != 1 {
        return;
    }
    s.set_cfg_buscando_cnpj(true);
    let (server, cnpj) = (ctx.server.clone(), so_digitos(&mascarado));
    ctx.spawn(async move { chamar(server, "GET", &format!("/consulta-cnpj/{cnpj}"), Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_cfg_buscando_cnpj(false);
        if !r.ok() {
            return;
        }
        let d = r.json();
        let usa = |k: &str, set: &dyn Fn(SharedString)| {
            let v = texto(&d, k);
            if !v.is_empty() {
                set(v.into());
            }
        };
        usa("razaoSocial", &|v| s.set_cfg_razao(v));
        usa("nomeFantasia", &|v| s.set_cfg_fantasia(v));
        usa("logradouro", &|v| s.set_cfg_logradouro(v));
        usa("numero", &|v| s.set_cfg_numero(v));
        usa("bairro", &|v| s.set_cfg_bairro(v));
        usa("municipio", &|v| s.set_cfg_municipio(v));
        usa("uf", &|v| s.set_cfg_uf(v));
        let cep = texto(&d, "cep");
        if !cep.is_empty() {
            s.set_cfg_cep(mascara_cep(&cep).into());
        }
    });
}

fn json_config(s: &FiscalStore<'_>) -> Value {
    json!({
        "apiToken": s.get_cfg_token().trim(),
        "razaoSocial": s.get_cfg_razao().as_str(),
        "nomeFantasia": s.get_cfg_fantasia().as_str(),
        "cnpj": s.get_cfg_cnpj().as_str(),
        "ie": s.get_cfg_ie().as_str(),
        "crt": s.get_cfg_crt().as_str(),
        "cscId": s.get_cfg_csc_id().as_str(),
        "cscSecret": s.get_cfg_csc().as_str(),
        "environment": s.get_cfg_ambiente().as_str(),
        "serieNfe": s.get_cfg_serie_nfe().as_str(),
        "proximoNumeroNfe": s.get_cfg_proximo_nfe().as_str(),
        "serieNfce": s.get_cfg_serie_nfce().as_str(),
        "proximoNumeroNfce": s.get_cfg_proximo_nfce().as_str(),
        "cep": s.get_cfg_cep().as_str(),
        "logradouro": s.get_cfg_logradouro().as_str(),
        "numero": s.get_cfg_numero().as_str(),
        "bairro": s.get_cfg_bairro().as_str(),
        "municipio": s.get_cfg_municipio().as_str(),
        "uf": s.get_cfg_uf().as_str(),
    })
}

fn salvar_config(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    if !s.get_cfg_cnpj().is_empty() && s.get_cfg_cnpj_valido() == 0 {
        s.set_cfg_mensagem_ok(false);
        s.set_cfg_mensagem("CNPJ inválido! Verifique os dígitos antes de salvar.".into());
        return;
    }
    let mut campos = vec![barerp_server::CampoMultipart::texto("settings", &json_config(&s).to_string())];
    if let Some((nome, bytes)) = CERTIFICADO.with(|c| c.borrow().clone()) {
        campos.push(barerp_server::CampoMultipart::arquivo("certificado", &nome, bytes));
    }
    let senha = s.get_cfg_cert_senha().to_string();
    if !senha.is_empty() {
        campos.push(barerp_server::CampoMultipart::texto("certPassword", &senha));
    }
    s.set_cfg_salvando(true);
    s.set_cfg_mensagem(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "PUT", "/settings", Corpo::Multipart(campos)).await }, |ui, r| {
        let s = store_de(ui);
        s.set_cfg_salvando(false);
        s.set_cfg_mensagem_ok(r.ok());
        s.set_cfg_mensagem(
            if r.ok() {
                "Configurações Fiscais salvas! A empresa está ativa na SEFAZ.".to_string()
            } else {
                Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao salvar e ativar empresa.".into())
            }
            .into(),
        );
    });
}

fn testar_conexao(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_cfg_validando(true);
    s.set_cfg_validacao(FxValidacao::default());
    s.set_cfg_diagnostico(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", "/validate-api", Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_cfg_validando(false);
        let d = r.json();
        let ok = d.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let erro = Some(texto(&d, "error"))
            .filter(|e| !e.is_empty())
            .unwrap_or_else(|| if d.is_null() { "Sem resposta do servidor. Verifique se o BarERP está rodando.".into() } else { String::new() });
        let status_cnpj = texto(&d, "empresaCadastrada");
        s.set_cfg_validacao(FxValidacao {
            visivel: true,
            ok,
            ambiente: texto(&d, "ambiente").into(),
            total: texto(&d, "totalEmpresas").into(),
            nao_localizado: status_cnpj.contains("não localizado"),
            status_cnpj: status_cnpj.into(),
            erro_token: erro.contains("Token"),
            erro: erro.into(),
        });
    });
}

fn diagnosticar(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    store_de(&ui).set_cfg_diagnosticando(true);
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", "/diagnose-focus", Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_cfg_diagnosticando(false);
        let d = r.json();
        let texto_diag = if !r.ok() {
            format!("🔍 Diagnóstico Focus NFe\n{}", Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Sem resposta do servidor.".into()))
        } else {
            let bonito = |v: Option<&Value>| v.map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default();
            let cnpj = Some(texto(&d, "cnpjConfigurado")).filter(|c| !c.is_empty());
            format!(
                "🔍 Diagnóstico Focus NFe — {}\n\nCNPJ configurado: {}\n\nGET /v2/empresas — Status {}\n{}\n\nGET /v2/empresas/{} — Status {}\n{}",
                texto(&d, "ambiente"),
                cnpj.clone().unwrap_or_else(|| "(não configurado)".into()),
                texto(&d, "listaEmpresasStatus"),
                bonito(d.get("listaEmpresasResposta")),
                cnpj.unwrap_or_default(),
                texto(&d, "buscaDiretaStatus"),
                bonito(d.get("buscaDiretaResposta")),
            )
        };
        s.set_cfg_diagnostico(texto_diag.into());
    });
}

fn escolher_certificado(ctx: &AppCtx) {
    ctx.spawn(
        async {
            let caminho = tokio::task::spawn_blocking(|| arquivos::abrir("Certificado Digital A1 (.pfx / .p12)", &["pfx", "p12"]))
                .await
                .ok()
                .flatten()?;
            let bytes = std::fs::read(&caminho).ok()?;
            let nome = caminho.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "certificado.pfx".into());
            Some((nome, bytes))
        },
        |ui, r| {
            let Some((nome, bytes)) = r else { return };
            let s = store_de(ui);
            s.set_cfg_cert_arquivo(format!("{nome} ({:.1} KB)", bytes.len() as f64 / 1024.0).into());
            s.set_cfg_cert_resultado(SharedString::new());
            CERTIFICADO.with(|c| *c.borrow_mut() = Some((nome, bytes)));
        },
    );
}

fn enviar_certificado(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let Some((nome, bytes)) = CERTIFICADO.with(|c| c.borrow().clone()) else { return };
    let senha = s.get_cfg_cert_senha().to_string();
    if senha.is_empty() {
        return;
    }
    s.set_cfg_enviando_cert(true);
    s.set_cfg_cert_resultado(SharedString::new());
    let campos = vec![
        barerp_server::CampoMultipart::arquivo("certificado", &nome, bytes),
        barerp_server::CampoMultipart::texto("certPassword", &senha),
    ];
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "POST", "/upload-cert", Corpo::Multipart(campos)).await }, |ui, r| {
        let s = store_de(ui);
        s.set_cfg_enviando_cert(false);
        s.set_cfg_cert_ok(r.ok());
        let d = r.json();
        s.set_cfg_cert_resultado(
            if r.ok() {
                format!("{}\nArquivo: {} • {}", texto(&d, "mensagem"), texto(&d, "arquivo"), texto(&d, "tamanho"))
            } else {
                format!("❌ {}", Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro desconhecido.".into()))
            }
            .into(),
        );
    });
}

// ============================================================
// Emissor de NFC-e em tela cheia (ManualNfceView)
// ============================================================

#[derive(Default)]
struct Emissor {
    produtos: Vec<barerp_db::importacao::ProdutoFiscal>,
    /// Ids dos produtos achados pela busca (na ordem da lista).
    resultados: Vec<String>,
    itens: Vec<(barerp_db::importacao::ProdutoFiscal, i32)>,
    /// Produtos sem NCM válido esperando o cadastro na hora, e o que foi digitado para cada um.
    sem_ncm: Vec<barerp_db::importacao::ProdutoFiscal>,
    ncm_digitado: Vec<String>,
    /// Referência da última nota emitida (imprimir, DANFE, e-mail).
    referencia: String,
    /// O cadastro de NCM foi pedido pela tela de NF-e (senão, NFC-e).
    depois_do_ncm_emite_nfe: bool,
}

/// NCM válido para a SEFAZ: 8 dígitos (a pontuação é ignorada).
fn ncm_valido(ncm: &str) -> bool {
    so_digitos(ncm).len() == 8
}

fn total_emissor() -> f64 {
    EMISSOR.with(|e| e.borrow().itens.iter().map(|(p, q)| p.price * f64::from(*q)).sum())
}

fn filtrar_produtos(ctx: &AppCtx, termo: &str) {
    let Some(ui) = ctx.ui() else { return };
    let termo = termo.trim().to_lowercase();
    let achados: Vec<barerp_db::importacao::ProdutoFiscal> = if termo.is_empty() {
        Vec::new()
    } else {
        EMISSOR.with(|e| {
            e.borrow()
                .produtos
                .iter()
                .filter(|p| {
                    let tem = |v: &Option<String>| v.as_deref().is_some_and(|v| v.to_lowercase().contains(&termo));
                    p.name.to_lowercase().contains(&termo) || tem(&p.code) || tem(&p.ean) || tem(&p.brand)
                })
                .cloned()
                .collect()
        })
    };
    let s = store_de(&ui);
    s.set_nf_qtd_encontrados(achados.len() as i32);
    let mostrados: Vec<_> = achados.iter().take(10).collect();
    EMISSOR.with(|e| e.borrow_mut().resultados = mostrados.iter().map(|p| p.id.clone()).collect());
    let linhas: Vec<FxProdutoBusca> = mostrados
        .iter()
        .map(|p| FxProdutoBusca {
            nome: p.name.clone().into(),
            codigo: p.code.clone().unwrap_or_default().into(),
            marca: p.brand.clone().unwrap_or_default().into(),
            ncm: p.ncm.clone().unwrap_or_default().into(),
            cfop: p.cfop.clone().unwrap_or_default().into(),
            preco: reais(p.price).into(),
        })
        .collect();
    s.set_nf_resultados(slint::ModelRc::new(slint::VecModel::from(linhas)));
}

/// Enter na busca: EAN/código exato adiciona; um resultado adiciona; vários abrem a lista.
fn busca_enter(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let termo = s.get_nf_busca().trim().to_lowercase();
    if termo.is_empty() {
        return;
    }
    let exato = EMISSOR.with(|e| {
        e.borrow()
            .produtos
            .iter()
            .find(|p| {
                let igual = |v: &Option<String>| v.as_deref().is_some_and(|v| v.to_lowercase() == termo);
                igual(&p.ean) || igual(&p.code)
            })
            .map(|p| p.id.clone())
    });
    if let Some(id) = exato {
        return adicionar_produto(ctx, id);
    }
    filtrar_produtos(ctx, &termo);
    let resultados = EMISSOR.with(|e| e.borrow().resultados.clone());
    match s.get_nf_qtd_encontrados() {
        1 => adicionar_produto(ctx, resultados[0].clone()),
        n if n > 1 => s.set_nf_lista_aberta(true),
        _ => {}
    }
}

fn adicionar_produto(ctx: &AppCtx, id: String) {
    EMISSOR.with(|e| {
        let mut e = e.borrow_mut();
        if let Some(item) = e.itens.iter_mut().find(|(p, _)| p.id == id) {
            item.1 += 1;
        } else if let Some(p) = e.produtos.iter().find(|p| p.id == id).cloned() {
            e.itens.push((p, 1));
        }
    });
    if let Some(ui) = ctx.ui() {
        let s = store_de(&ui);
        s.set_nf_busca(SharedString::new());
        s.set_nf_lista_aberta(false);
    }
    mostrar_itens(ctx);
}

fn mostrar_itens(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let (linhas, unidades) = EMISSOR.with(|e| {
        let e = e.borrow();
        let linhas: Vec<FxItemNfce> = e
            .itens
            .iter()
            .map(|(p, q)| FxItemNfce {
                nome: p.name.clone().into(),
                codigo: p.code.clone().unwrap_or_default().into(),
                ncm_ok: p.ncm.as_deref().is_some_and(ncm_valido),
                ncm: p.ncm.clone().unwrap_or_default().into(),
                cfop: p.cfop.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "5102".into()).into(),
                qtd: *q,
                unitario: reais(p.price).into(),
                subtotal: reais(p.price * f64::from(*q)).into(),
            })
            .collect();
        (linhas, e.itens.iter().map(|(_, q)| *q).sum::<i32>())
    });
    s.set_nf_itens(slint::ModelRc::new(slint::VecModel::from(linhas)));
    s.set_nf_unidades(unidades);
    s.set_nf_total(reais(total_emissor()).into());
    validar_documento(ctx);
}

/// Documento do consumidor: dígitos verificadores e obrigatoriedade a partir de R$ 10.000,00.
fn validar_documento(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let doc = s.get_nf_doc().to_string();
    let limpo = so_digitos(&doc);
    let obrigatorio = total_emissor() >= barerp_core::fiscal_doc::NFCE_LIMITE_SEM_IDENTIFICACAO;
    s.set_nf_doc_obrigatorio(obrigatorio);
    let erro = barerp_core::fiscal_doc::cpf_cnpj_error(&doc).map(str::to_string).or_else(|| {
        (obrigatorio && limpo.is_empty()).then(|| "Obrigatório: a SEFAZ exige CPF ou CNPJ em NFC-e a partir de R$ 10.000,00.".to_string())
    });
    s.set_nf_doc_erro(erro.clone().unwrap_or_default().into());
    s.set_nf_doc_ok(
        if erro.is_none() && !limpo.is_empty() { if limpo.len() == 11 { "CPF válido" } else { "CNPJ válido" } } else { "" }.into(),
    );
}

fn aviso_emissor(ctx: &AppCtx, msg: impl Into<SharedString>) {
    if let Some(ui) = ctx.ui() {
        store_de(&ui).set_nf_aviso(msg.into());
    }
}

fn emitir_nfce_manual(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let itens = EMISSOR.with(|e| e.borrow().itens.clone());
    if itens.is_empty() {
        return aviso_emissor(ctx, "Adicione pelo menos um produto à nota fiscal.");
    }
    let doc = s.get_nf_doc().to_string();
    if let Some(erro) = barerp_core::fiscal_doc::cpf_cnpj_error(&doc) {
        return aviso_emissor(ctx, format!("{erro} Corrija o documento ou deixe o campo em branco."));
    }
    let limpo = so_digitos(&doc);
    let total = total_emissor();
    if total >= barerp_core::fiscal_doc::NFCE_LIMITE_SEM_IDENTIFICACAO && limpo.is_empty() {
        return aviso_emissor(ctx, "Para NFC-e a partir de R$ 10.000,00 a SEFAZ exige o CPF ou CNPJ do consumidor.");
    }
    // produtos sem NCM: cadastra na hora (fica salvo para as próximas vendas)
    let sem_ncm: Vec<_> = itens.iter().filter(|(p, _)| !p.ncm.as_deref().is_some_and(ncm_valido)).map(|(p, _)| p.clone()).collect();
    if !sem_ncm.is_empty() {
        EMISSOR.with(|e| {
            let mut e = e.borrow_mut();
            e.ncm_digitado = vec![String::new(); sem_ncm.len()];
            e.sem_ncm = sem_ncm;
        });
        return mostrar_pedidos_ncm(ctx);
    }
    let forma = s.get_nf_pagamento().to_string();
    let nome = s.get_nf_nome().trim().to_string();
    let corpo = json!({
        "orderId": format!("{}{}", chrono::Utc::now().timestamp_millis(), rand_quatro_digitos()),
        "customerCpf": if limpo.is_empty() { None } else { Some(&limpo) },
        "customerName": if nome.is_empty() { None } else { Some(&nome) },
        "paymentMethod": forma,
        "items": itens.iter().map(|(p, q)| json!({
            "productId": p.id,
            "ean": p.ean,
            "code": p.code,
            "quantity": q,
            "price": p.price,
            "name": p.name,
            "ncm": so_digitos(p.ncm.as_deref().unwrap_or("")),
            "cfop": p.cfop.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "5102".into()),
            "unit": if p.unit.is_empty() { "un".to_string() } else { p.unit.clone() },
            "cest": p.cest.clone().filter(|c| !c.is_empty()),
        })).collect::<Vec<_>>(),
    });
    s.set_nf_emitindo(true);
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", "/emit-nfce", Corpo::Json(corpo)).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_nf_emitindo(false);
        let d = r.json();
        if !r.ok() {
            return aviso_emissor(&c, Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro na transmissão à SEFAZ.".into()));
        }
        let referencia = Some(texto(&d, "referencia")).filter(|r| !r.is_empty()).unwrap_or_else(|| texto(&d, "ref"));
        if texto(&d, "status") != "autorizado" {
            nova_venda(&c);
            return aviso_emissor(
                &c,
                format!(
                    "A NFC-e foi enviada e está em processamento na SEFAZ (ref: {referencia}).\n\nNÃO emita novamente. Consulte em instantes na tela de Reimpressão de NFC-e."
                ),
            );
        }
        EMISSOR.with(|e| e.borrow_mut().referencia = referencia.clone());
        let forma = match forma.as_str() {
            "DINHEIRO" => "Dinheiro".to_string(),
            "CREDITO" => "Cartão Crédito".to_string(),
            "DEBITO" => "Cartão Débito".to_string(),
            outra => outra.to_string(),
        };
        s.set_nf_email(SharedString::new());
        s.set_nf_email_ok(false);
        s.set_nf_email_erro(SharedString::new());
        s.set_nf_venda(FxVendaOk { visivel: true, total: reais(total).into(), forma: forma.into(), chave: texto(&d, "chaveAcesso").into(), impresso: true });
        // impressão automática do cupom, como o Electron fazia
        imprimir_danfe_nfce(&c, &referencia);
    });
}

fn rand_quatro_digitos() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    1000 + n % 9000
}

fn mostrar_pedidos_ncm(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let (pedidos, pronto) = EMISSOR.with(|e| {
        let e = e.borrow();
        let pedidos: Vec<FxNcmPedido> = e
            .sem_ncm
            .iter()
            .zip(&e.ncm_digitado)
            .map(|(p, v)| FxNcmPedido { nome: p.name.clone().into(), valor: v.clone().into(), valido: v.len() == 8 })
            .collect();
        let pronto = !e.ncm_digitado.is_empty() && e.ncm_digitado.iter().all(|v| v.len() == 8);
        (pedidos, pronto)
    });
    let s = store_de(&ui);
    s.set_nf_ncm(slint::ModelRc::new(slint::VecModel::from(pedidos)));
    s.set_nf_ncm_pronto(pronto);
}

fn salvar_ncm_e_emitir(ctx: &AppCtx) {
    let (pedidos, digitados) = EMISSOR.with(|e| {
        let e = e.borrow();
        (e.sem_ncm.clone(), e.ncm_digitado.clone())
    });
    if let Some((p, _)) = pedidos.iter().zip(&digitados).find(|(_, v)| v.len() != 8) {
        return aviso_emissor(ctx, format!("Informe o NCM com 8 dígitos para \"{}\".", p.name));
    }
    if let Some(ui) = ctx.ui() {
        store_de(&ui).set_nf_ncm_salvando(true);
    }
    let db = ctx.db.clone();
    let lista: Vec<(String, String)> = pedidos.iter().map(|p| p.id.clone()).zip(digitados).collect();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            for (id, ncm) in &lista {
                barerp_db::importacao::salvar_ncm(&db, id, ncm).await.map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(lista)
        },
        move |ui, r| {
            store_de(ui).set_nf_ncm_salvando(false);
            match r {
                Ok(lista) => {
                    EMISSOR.with(|e| {
                        let mut e = e.borrow_mut();
                        for (id, ncm) in &lista {
                            for p in e.produtos.iter_mut().filter(|p| &p.id == id) {
                                p.ncm = Some(ncm.clone());
                            }
                            for (p, _) in e.itens.iter_mut().filter(|(p, _)| &p.id == id) {
                                p.ncm = Some(ncm.clone());
                            }
                        }
                        e.sem_ncm.clear();
                        e.ncm_digitado.clear();
                    });
                    mostrar_pedidos_ncm(&c);
                    mostrar_itens(&c);
                    if EMISSOR.with(|e| std::mem::take(&mut e.borrow_mut().depois_do_ncm_emite_nfe)) {
                        emitir_nfe(&c);
                    } else {
                        emitir_nfce_manual(&c);
                    }
                }
                Err(e) => aviso_emissor(&c, if e.is_empty() { "Erro ao salvar o NCM do produto.".to_string() } else { e }),
            }
        },
    );
}

fn enviar_email_nfce(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let email = s.get_nf_email().trim().to_string();
    if email.is_empty() || !email.contains('@') {
        s.set_nf_email_erro("Digite um endereço de e-mail válido.".into());
        return;
    }
    let referencia = EMISSOR.with(|e| e.borrow().referencia.clone());
    if referencia.is_empty() {
        s.set_nf_email_erro("Referência da nota não localizada para envio.".into());
        return;
    }
    s.set_nf_enviando_email(true);
    s.set_nf_email_erro(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(
        async move { chamar(server, "POST", "/send-email", Corpo::Json(json!({ "referencia": referencia, "email": email }))).await },
        |ui, r| {
            let s = store_de(ui);
            s.set_nf_enviando_email(false);
            if r.ok() {
                s.set_nf_email_ok(true);
            } else {
                s.set_nf_email_erro(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Falha no envio do e-mail.".into()).into());
            }
        },
    );
}

fn nova_venda(ctx: &AppCtx) {
    EMISSOR.with(|e| {
        let mut e = e.borrow_mut();
        e.itens.clear();
        e.referencia.clear();
    });
    if let Some(ui) = ctx.ui() {
        let s = store_de(&ui);
        s.set_nf_venda(FxVendaOk::default());
        s.set_nf_busca(SharedString::new());
        s.set_nf_doc(SharedString::new());
        s.set_nf_nome(SharedString::new());
        s.set_nf_email(SharedString::new());
        s.set_nf_email_ok(false);
        s.set_nf_email_erro(SharedString::new());
    }
    mostrar_itens(ctx);
}

// ============================================================
// Emissão de NF-e Modelo 55 (EmitNfeView)
// ============================================================

thread_local! {
    /// Consulta a nota em processamento a cada 2,5 s até a chave chegar (como o modal do original).
    static TIMER_NFE: Timer = Timer::default();
}

fn limpar_destinatario(s: &FiscalStore<'_>) {
    s.set_ne_doc(SharedString::new());
    s.set_ne_nome(SharedString::new());
    s.set_ne_cep(SharedString::new());
    s.set_ne_logradouro(SharedString::new());
    s.set_ne_numero(SharedString::new());
    s.set_ne_complemento(SharedString::new());
    s.set_ne_bairro(SharedString::new());
    s.set_ne_municipio(SharedString::new());
    s.set_ne_uf(SharedString::new());
    s.set_ne_doc_msg(SharedString::new());
    s.set_ne_doc_detalhe(SharedString::new());
    s.set_ne_doc_estado(-1);
}

fn feedback_doc(ctx: &AppCtx, estado: i32, msg: &str, detalhe: &str) {
    if let Some(ui) = ctx.ui() {
        let s = store_de(&ui);
        s.set_ne_doc_estado(estado);
        s.set_ne_doc_msg(msg.into());
        s.set_ne_doc_detalhe(detalhe.into());
    }
}

/// Validação do CPF/CNPJ enquanto digita; CPF procura o cliente cadastrado, CNPJ consulta a Receita.
fn documento_nfe(ctx: &AppCtx, digitado: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mascarado = barerp_core::fiscal_doc::format_cpf_cnpj(digitado);
    if mascarado != digitado {
        s.set_ne_doc(mascarado.into());
    }
    let limpo = so_digitos(digitado);
    match limpo.len() {
        11 if barerp_core::fiscal_doc::validate_cpf(&limpo) => {
            feedback_doc(ctx, 1, "CPF Válido", "");
            let (db, c) = (ctx.db.clone(), ctx.clone());
            ctx.spawn(async move { barerp_db::customers::list(&db).await }, move |ui, r| {
                let Ok(clientes) = r else { return };
                if let Some(cli) = clientes.iter().find(|c| c.document.as_deref().is_some_and(|d| so_digitos(d) == limpo)) {
                    store_de(ui).set_ne_nome(cli.name.clone().into());
                    feedback_doc(&c, 1, "CPF Válido", &format!("Cliente Cadastrado: {}", cli.name));
                }
            });
        }
        11 => feedback_doc(ctx, 0, "CPF Inválido (dígitos verificadores incorretos)", ""),
        14 if barerp_core::fiscal_doc::validate_cnpj(&limpo) => {
            feedback_doc(ctx, 1, "CNPJ Válido. Buscando dados e endereço na Receita...", "");
            consultar_cnpj_receita(ctx, limpo);
        }
        14 => feedback_doc(ctx, 0, "CNPJ Inválido (dígitos verificadores incorretos)", ""),
        0 => feedback_doc(ctx, -1, "", ""),
        n => {
            let (tipo, total) = if n <= 11 { ("CPF", 11) } else { ("CNPJ", 14) };
            feedback_doc(ctx, 2, &format!("Digitando {tipo} ({n} de {total} dígitos)..."), "");
        }
    }
}

/// Dados e endereço do CNPJ na Receita Federal (rota /consulta-cnpj).
fn consultar_cnpj_receita(ctx: &AppCtx, cnpj: String) {
    let Some(ui) = ctx.ui() else { return };
    store_de(&ui).set_ne_buscando_doc(true);
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "GET", &format!("/consulta-cnpj/{cnpj}"), Corpo::Nada).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_ne_buscando_doc(false);
        let d = r.json();
        let nome = Some(texto(&d, "razaoSocial")).filter(|n| !n.is_empty()).unwrap_or_else(|| texto(&d, "nomeFantasia"));
        if r.status == 503 || d.is_null() {
            return feedback_doc(&c, 1, "CNPJ Válido", "Não foi possível consultar a Receita Federal agora. Digite a Razão Social e Endereço manualmente.");
        }
        if !r.ok() || nome.is_empty() {
            let det = Some(texto(&d, "error")).filter(|e| !e.is_empty()).unwrap_or_else(|| "Dados da empresa não localizados automaticamente. Preencha abaixo.".into());
            return feedback_doc(&c, 1, "CNPJ Válido", &det);
        }
        s.set_ne_nome(nome.clone().into());
        let usa = |k: &str, set: &dyn Fn(SharedString)| {
            let v = texto(&d, k);
            if !v.is_empty() {
                set(v.into());
            }
        };
        let cep = texto(&d, "cep");
        if !cep.is_empty() {
            s.set_ne_cep(mascara_cep(&cep).into());
        }
        usa("logradouro", &|v| s.set_ne_logradouro(v));
        usa("numero", &|v| s.set_ne_numero(v));
        usa("complemento", &|v| s.set_ne_complemento(v));
        usa("bairro", &|v| s.set_ne_bairro(v));
        usa("municipio", &|v| s.set_ne_municipio(v));
        usa("uf", &|v| s.set_ne_uf(v));
        let local = [texto(&d, "municipio"), texto(&d, "uf")].into_iter().filter(|v| !v.is_empty()).collect::<Vec<_>>().join("/");
        let det = if local.is_empty() { nome } else { format!("{nome} • {local}") };
        feedback_doc(&c, 1, "CNPJ e Endereço Localizados na Receita Federal", &det);
    });
}

/// Máscara do CEP e, com 8 dígitos, o endereço pela BrasilAPI ou ViaCEP.
fn cep_editado(ctx: &AppCtx, digitado: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mascarado = mascara_cep(digitado);
    if mascarado != digitado {
        s.set_ne_cep(mascarado.clone().into());
    }
    let cep = so_digitos(&mascarado);
    if cep.len() != 8 {
        return;
    }
    s.set_ne_buscando_cep(true);
    ctx.spawn(
        async move {
            barerp_fiscal::publico::consultar_cep(barerp_fiscal::publico::BRASIL_API, barerp_fiscal::publico::VIA_CEP, &cep).await
        },
        |ui, e| {
            let s = store_de(ui);
            s.set_ne_buscando_cep(false);
            let Some(e) = e else { return };
            let usa = |v: &str, set: &dyn Fn(SharedString)| {
                if !v.is_empty() {
                    set(v.into());
                }
            };
            usa(&e.logradouro, &|v| s.set_ne_logradouro(v));
            usa(&e.bairro, &|v| s.set_ne_bairro(v));
            usa(&e.municipio, &|v| s.set_ne_municipio(v));
            usa(&e.uf, &|v| s.set_ne_uf(v));
        },
    );
}

fn emitir_nfe(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let itens = EMISSOR.with(|e| e.borrow().itens.clone());
    if itens.is_empty() {
        return aviso_emissor(ctx, "Adicione pelo menos um produto à nota fiscal.");
    }
    let doc = so_digitos(&s.get_ne_doc());
    if doc.is_empty() {
        return aviso_emissor(ctx, "Atenção: A SEFAZ exige obrigatoriamente a identificação do comprador (CPF ou CNPJ) para emissão de NF-e (Modelo 55).");
    }
    if doc.len() == 11 && !barerp_core::fiscal_doc::validate_cpf(&doc) {
        return aviso_emissor(ctx, "O CPF informado possui dígitos verificadores inválidos. Por favor, corrija o documento digitado antes de emitir.");
    }
    if doc.len() == 14 && !barerp_core::fiscal_doc::validate_cnpj(&doc) {
        return aviso_emissor(ctx, "O CNPJ informado possui dígitos verificadores inválidos. Por favor, corrija o documento digitado antes de emitir.");
    }
    if doc.len() != 11 && doc.len() != 14 {
        return aviso_emissor(ctx, "Documento incompleto. Digite um CPF válido com 11 dígitos ou um CNPJ com 14 dígitos.");
    }
    let nome = s.get_ne_nome().trim().to_string();
    if nome.chars().count() < 2 {
        return aviso_emissor(ctx, "Informe o Nome ou Razão Social do comprador para a NF-e. Este campo é obrigatório pela SEFAZ.");
    }
    // NCM obrigatório: o original mandava 22030000 para produto sem NCM; aqui é cadastrado na hora
    let sem_ncm: Vec<_> = itens.iter().filter(|(p, _)| !p.ncm.as_deref().is_some_and(ncm_valido)).map(|(p, _)| p.clone()).collect();
    if !sem_ncm.is_empty() {
        EMISSOR.with(|e| {
            let mut e = e.borrow_mut();
            e.ncm_digitado = vec![String::new(); sem_ncm.len()];
            e.sem_ncm = sem_ncm;
            e.depois_do_ncm_emite_nfe = true;
        });
        return mostrar_pedidos_ncm(ctx);
    }
    let opcional = |v: SharedString| Some(v.trim().to_string()).filter(|v| !v.is_empty());
    let forma = s.get_nf_pagamento().to_string();
    let total = total_emissor();
    let corpo = json!({
        "orderId": format!("{}{}", chrono::Utc::now().timestamp_millis(), rand_quatro_digitos()),
        "customerCpf": doc,
        "customerName": nome,
        "paymentMethod": forma,
        "customerCep": Some(so_digitos(&s.get_ne_cep())).filter(|c| !c.is_empty()),
        "customerLogradouro": opcional(s.get_ne_logradouro()),
        "customerNumero": opcional(s.get_ne_numero()),
        "customerComplemento": opcional(s.get_ne_complemento()),
        "customerBairro": opcional(s.get_ne_bairro()),
        "customerMunicipio": opcional(s.get_ne_municipio()),
        "customerUf": opcional(s.get_ne_uf()),
        "items": itens.iter().map(|(p, q)| json!({
            "productId": p.id,
            "ean": p.ean,
            "code": p.code,
            "quantity": q,
            "price": p.price,
            "name": p.name,
            "ncm": so_digitos(p.ncm.as_deref().unwrap_or("")),
            "cfop": p.cfop.clone().filter(|c| !c.is_empty()).unwrap_or_else(|| "5102".into()),
            "unit": if p.unit.is_empty() { "un".to_string() } else { p.unit.clone() },
            "cest": p.cest.as_deref().and_then(barerp_core::fiscal_doc::sanitize_cest),
        })).collect::<Vec<_>>(),
    });
    s.set_nf_emitindo(true);
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", "/emit-nfe", Corpo::Json(corpo)).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_nf_emitindo(false);
        if !r.ok() {
            return aviso_emissor(&c, Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao emitir NF‑e".into()));
        }
        let d = r.json();
        let referencia = Some(texto(&d, "referencia")).filter(|r| !r.is_empty()).unwrap_or_else(|| texto(&d, "ref"));
        let chave = Some(texto(&d, "chaveAcesso")).filter(|r| !r.is_empty()).unwrap_or_else(|| texto(&d, "chave"));
        EMISSOR.with(|e| e.borrow_mut().referencia = referencia.clone());
        s.set_nf_email(SharedString::new());
        s.set_nf_email_ok(false);
        s.set_nf_email_erro(SharedString::new());
        s.set_ne_nota(FxNfeOk { visivel: true, referencia: referencia.into(), total: reais(total).into(), forma: forma.into(), chave: chave.clone().into(), impresso: false });
        if chave.is_empty() {
            acompanhar_nfe(&c);
        }
    });
}

/// Enquanto a SEFAZ processa, consulta a nota até a chave chegar.
fn acompanhar_nfe(ctx: &AppCtx) {
    let c = ctx.clone();
    TIMER_NFE.with(|t| {
        t.start(TimerMode::Repeated, std::time::Duration::from_millis(2500), move || {
            let Some(ui) = c.ui() else { return };
            let nota = store_de(&ui).get_ne_nota();
            if !nota.visivel || !nota.chave.is_empty() {
                TIMER_NFE.with(|t| t.stop());
                return;
            }
            let (server, caminho) = (c.server.clone(), format!("/nfe/reprint/{}", uri_seg(&nota.referencia)));
            c.spawn(async move { chamar(server, "GET", &caminho, Corpo::Nada).await.json() }, |ui, d| {
                let chave = Some(texto(&d, "chaveAcesso")).filter(|k| !k.is_empty()).or_else(|| d.get("nota").map(|n| texto(n, "chave")).filter(|k| !k.is_empty()));
                if let Some(chave) = chave {
                    let s = store_de(ui);
                    let mut n = s.get_ne_nota();
                    n.chave = chave.into();
                    s.set_ne_nota(n);
                }
            });
        });
    });
}

fn nova_nfe(ctx: &AppCtx) {
    TIMER_NFE.with(|t| t.stop());
    nova_venda(ctx);
    if let Some(ui) = ctx.ui() {
        let s = store_de(&ui);
        s.set_ne_nota(FxNfeOk::default());
        limpar_destinatario(&s);
    }
}

// ============================================================
// 2º BIP: Importação de NF-e e entrada no estoque (FiscalImportView)
// ============================================================

#[derive(Clone, Default)]
struct ItemXmlNota {
    code: String,
    name: String,
    quantity: f64,
    unit_cost: f64,
    ncm: String,
    cfop: String,
    unit: String,
    ean: String,
}

#[derive(Clone, Default)]
struct Vinculo {
    item: ItemXmlNota,
    /// 0 vincular, 1 criar, 2 ignorar
    acao: i32,
    produto: Option<usize>,
    categoria: Option<usize>,
    margem: f64,
    margem_txt: String,
    preco: f64,
    preco_txt: String,
    atualizar_caixa: bool,
    preco_caixa: Option<f64>,
    preco_caixa_txt: String,
    fator: f64,
    fator_txt: String,
    conv_ativo: bool,
    /// 0 caixa, 1 porções
    conv_modo: i32,
    /// Vincular com NCM diferente do cadastro: fica o do cadastro (senão vale o da nota, como no original)
    manter_ncm: bool,
}

#[derive(Default)]
struct Importacao {
    produtos: Vec<barerp_db::importacao::ProdutoFiscal>,
    categorias: Vec<(String, String)>,
    margem_padrao: f64,
    /// Nota carregada (resposta do /import-xml ou /parsed-by-chave).
    nota: Value,
    itens: Vec<Vinculo>,
    /// Chave com que a tela foi aberta (Recebimento/Pendentes); o 2º bip tem de conferir com ela.
    chave_origem: String,
    arquivo: Option<(String, Vec<u8>)>,
}

fn arred(v: f64, casas: i32) -> f64 {
    let f = 10f64.powi(casas);
    (v * f).round() / f
}

/// Preço pela margem: custo / (1 - margem%); margem de 100% ou mais dobra o custo.
fn preco_pela_margem(custo: f64, margem: f64) -> f64 {
    if custo <= 0.0 {
        0.0
    } else if margem >= 100.0 {
        arred(custo * 2.0, 2)
    } else {
        arred(custo / (1.0 - margem / 100.0), 2)
    }
}

fn margem_pelo_preco(custo: f64, venda: f64) -> f64 {
    if venda <= 0.0 { 0.0 } else { arred((venda - custo) / venda * 100.0, 1) }
}

fn num_br(t: &str) -> f64 {
    t.trim().replace(',', ".").parse().unwrap_or(0.0)
}

fn fmt2(v: f64) -> String {
    format!("{v:.2}")
}

fn fmt_num(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn custo_efetivo(v: &Vinculo) -> f64 {
    if v.conv_ativo && v.fator > 0.0 { arred(v.item.unit_cost / v.fator, 4) } else { v.item.unit_cost }
}

impl Importacao {
    fn produto(&self, v: &Vinculo) -> Option<&barerp_db::importacao::ProdutoFiscal> {
        v.produto.and_then(|i| self.produtos.get(i))
    }

    /// Caixa proporcional ao novo preço (ou preço × unidades da caixa).
    fn caixa_proporcional(&self, v: &Vinculo, preco: f64) -> Option<f64> {
        let p = self.produto(v)?;
        let qtd = p.box_quantity.filter(|q| *q > 0.0)?;
        if !p.has_box_price {
            return None;
        }
        Some(match p.box_price {
            Some(bp) if p.price > 0.0 && bp != 0.0 => arred(bp * (preco / p.price), 2),
            _ => arred(preco * qtd, 2),
        })
    }

    fn vinculo_inicial(&self, item: ItemXmlNota) -> Vinculo {
        let exato = self.produtos.iter().position(|p| p.name.trim().to_lowercase() == item.name.trim().to_lowercase());
        let prod = exato.and_then(|i| self.produtos.get(i));
        let margem = prod.and_then(|p| p.target_margin).filter(|m| *m > 0.0).unwrap_or(self.margem_padrao);
        let sugestao = barerp_fiscal::detectar_conversao(&item.name, &item.unit);
        let caixa_existente = prod.filter(|p| p.has_box_price).and_then(|p| p.box_quantity).filter(|q| *q > 1.0);
        let conv_ativo = caixa_existente.is_some() || sugestao.detectada;
        let fator = caixa_existente.unwrap_or(if sugestao.detectada { f64::from(sugestao.fator) } else { 1.0 });
        let custo = if conv_ativo && fator > 0.0 { item.unit_cost / fator } else { item.unit_cost };
        let preco = preco_pela_margem(custo, margem);
        let mut v = Vinculo {
            acao: if exato.is_some() { 0 } else { 1 },
            produto: exato,
            categoria: if self.categorias.is_empty() { None } else { Some(0) },
            margem,
            margem_txt: fmt_num(margem),
            preco,
            preco_txt: fmt2(preco),
            atualizar_caixa: prod.is_some_and(|p| p.has_box_price) || (conv_ativo && fator > 1.0),
            fator,
            fator_txt: fmt_num(fator),
            conv_ativo,
            conv_modo: if sugestao.modo == barerp_fiscal::ModoConversao::Porcao { 1 } else { 0 },
            item,
            ..Default::default()
        };
        v.preco_caixa = self.caixa_proporcional(&v, preco).or_else(|| (conv_ativo && fator > 1.0).then(|| arred(preco * fator, 2)));
        v.preco_caixa_txt = v.preco_caixa.map(fmt2).unwrap_or_default();
        v
    }

    fn linha(&self, i: usize) -> FxItemImport {
        let v = &self.itens[i];
        let it = &v.item;
        let prod = self.produto(v);
        let custo = custo_efetivo(v);
        let novo = v.preco;
        let (tendencia, tendencia_texto) = match prod {
            Some(p) if p.price > 0.0 && novo < p.price => (1, format!("Preço abaixará (-{})", reais(p.price - novo))),
            Some(p) if p.price > 0.0 && novo > p.price => (2, format!("Reajuste (+{})", reais(novo - p.price))),
            Some(_) => (3, "Preço mantido".to_string()),
            None => (0, String::new()),
        };
        let unidade_conv = if v.conv_modo == 1 { "porções" } else { "unidades" };
        let ncm_nota = so_digitos(&it.ncm);
        let ncm_produto = prod.and_then(|p| p.ncm.as_deref()).map(so_digitos).unwrap_or_default();
        FxItemImport {
            titulo: format!("ITEM #{}{}", i + 1, if it.code.is_empty() { String::new() } else { format!(" • CÓD: {}", it.code) }).into(),
            nome: it.name.clone().into(),
            qtd: format!("{} {}", fmt_num(it.quantity), it.unit).into(),
            custo: reais(it.unit_cost).into(),
            total: reais(it.quantity * it.unit_cost).into(),
            acao: v.acao,
            conv_ativo: v.conv_ativo,
            conv_modo: v.conv_modo,
            fator: v.fator_txt.clone().into(),
            custo_real: reais(custo).into(),
            entrada: format!("+{} {unidade_conv}", (it.quantity * if v.fator > 0.0 { v.fator } else { 1.0 }).round()).into(),
            custo_nf: if v.conv_ativo && v.fator > 1.0 { format!(" (NF: {} / {})", reais(it.unit_cost), fmt_num(v.fator)).into() } else { SharedString::new() },
            produto_idx: v.produto.map_or(-1, |p| p as i32),
            categoria_idx: v.categoria.map_or(-1, |c| c as i32),
            margem: v.margem_txt.clone().into(),
            preco: v.preco_txt.clone().into(),
            tendencia,
            tendencia_texto: tendencia_texto.into(),
            venda_atual: prod
                .map(|p| format!("Venda Atual: {} | Custo Ant.: {}", reais(p.price), p.cost_price.filter(|c| *c != 0.0).map(reais).unwrap_or_else(|| "-".into())))
                .unwrap_or_default()
                .into(),
            tem_caixa: prod.is_some_and(|p| p.has_box_price && p.box_quantity.is_some_and(|q| q > 0.0)),
            caixa_qtd: prod.and_then(|p| p.box_quantity).map(fmt_num).unwrap_or_default().into(),
            atualizar_caixa: v.atualizar_caixa,
            preco_caixa: v.preco_caixa_txt.clone().into(),
            caixa_atual: reais(prod.and_then(|p| p.box_price).unwrap_or(0.0)).into(),
            ncm: ncm_formatado(&ncm_nota).into(),
            ncm_produto: ncm_formatado(&ncm_produto).into(),
            ncm_conflito: ncm_em_conflito(v.acao, &ncm_nota, &ncm_produto),
            manter_ncm: v.manter_ncm,
            produto_nome: prod.map(|p| format!("{} (Estoque atual: {})", p.name, fmt_num(p.stock))).unwrap_or_default().into(),
        }
    }

    /// Produtos para vincular ao item `i`: com texto, os que têm todas as palavras no nome, código, código de barras
    /// ou marca (código de barras ou código exato primeiro); sem texto, os parecidos com o nome do item da nota
    /// primeiro (marcados como sugeridos) e depois o resto em ordem alfabética.
    fn busca_vinculo(&self, i: usize, termo: &str) -> Vec<FxProdutoAchado> {
        const LIMITE: usize = 80;
        let item = self.itens.get(i).map(|v| v.item.clone()).unwrap_or_default();
        let termo = crate::view::fold(termo.trim());
        let palavras: Vec<&str> = termo.split_whitespace().collect();
        let texto = |p: &barerp_db::importacao::ProdutoFiscal| {
            crate::view::fold(&format!(
                "{} {} {} {}",
                p.name,
                p.code.as_deref().unwrap_or(""),
                p.ean.as_deref().unwrap_or(""),
                p.brand.as_deref().unwrap_or("")
            ))
        };
        let mut achados: Vec<(usize, i32)> = if palavras.is_empty() {
            let da_nota: Vec<String> =
                crate::view::fold(&item.name).split_whitespace().filter(|w| w.chars().count() >= 3).map(str::to_string).collect();
            self.produtos
                .iter()
                .enumerate()
                .map(|(idx, p)| {
                    let nome = crate::view::fold(&p.name);
                    let mut pontos = da_nota.iter().filter(|w| nome.contains(w.as_str())).count() as i32;
                    if !item.ean.is_empty() && p.ean.as_deref() == Some(item.ean.as_str()) {
                        pontos += 100;
                    }
                    (idx, pontos)
                })
                .collect()
        } else {
            self.produtos
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    let t = texto(p);
                    palavras.iter().all(|w| t.contains(w))
                })
                .map(|(idx, p)| {
                    let exato = |v: &Option<String>| v.as_deref().is_some_and(|v| crate::view::fold(v) == termo);
                    (idx, if exato(&p.ean) || exato(&p.code) { 100 } else { 0 })
                })
                .collect()
        };
        // ordem estável: pontos e, no empate, a ordem alfabética que já vem do banco
        achados.sort_by_key(|(idx, pontos)| (-pontos, *idx));
        // "parecido com a nota" só nos mais parecidos (uma palavra em comum, como "CERV", não basta)
        let melhor = achados.first().map_or(0, |(_, pontos)| *pontos);
        achados
            .into_iter()
            .take(LIMITE)
            .map(|(idx, pontos)| {
                let p = &self.produtos[idx];
                let mut detalhe = Vec::new();
                if let Some(c) = p.code.as_deref().filter(|c| !c.is_empty()) {
                    detalhe.push(format!("Cód. {c}"));
                }
                if let Some(e) = p.ean.as_deref().filter(|e| !e.is_empty()) {
                    detalhe.push(format!("EAN {e}"));
                }
                detalhe.push(format!("Estoque {}", fmt_num(p.stock)));
                detalhe.push(format!("Venda {}", reais(p.price)));
                FxProdutoAchado {
                    idx: idx as i32,
                    nome: p.name.clone().into(),
                    detalhe: detalhe.join(" · ").into(),
                    sugerido: palavras.is_empty() && pontos > 0 && pontos == melhor,
                }
            })
            .collect()
    }
}

fn mostrar_busca_vinculo(ui: &AppWindow) {
    let s = store_de(ui);
    let (i, termo) = (s.get_im_busca_item(), s.get_im_busca());
    if i < 0 {
        return;
    }
    let achados = IMPORTACAO.with(|im| im.borrow().busca_vinculo(i as usize, &termo));
    s.set_im_busca_resultados(slint::ModelRc::new(slint::VecModel::from(achados)));
}

/// "22030000" → "2203.00.00" (o que não tem 8 dígitos fica como veio).
fn ncm_formatado(digitos: &str) -> String {
    if digitos.len() == 8 {
        format!("{}.{}.{}", &digitos[..4], &digitos[4..6], &digitos[6..])
    } else {
        digitos.to_string()
    }
}

/// Vincular a um produto que já tem NCM válido e diferente do da nota: o usuário escolhe qual fica.
fn ncm_em_conflito(acao: i32, ncm_nota: &str, ncm_produto: &str) -> bool {
    acao == 0 && !ncm_nota.is_empty() && ncm_valido(ncm_produto) && ncm_nota != ncm_produto
}

fn mostrar_conferencia(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let linhas: Vec<FxItemImport> = IMPORTACAO.with(|im| {
        let im = im.borrow();
        (0..im.itens.len()).map(|i| im.linha(i)).collect()
    });
    store_de(&ui).set_im_itens(slint::ModelRc::new(slint::VecModel::from(linhas)));
}

/// Atualiza só a linha do item (o campo em edição não perde o foco).
fn atualizar_item(ctx: &AppCtx, i: usize) {
    let Some(ui) = ctx.ui() else { return };
    let linha = IMPORTACAO.with(|im| {
        let im = im.borrow();
        (i < im.itens.len()).then(|| im.linha(i))
    });
    if let Some(linha) = linha {
        use slint::Model;
        store_de(&ui).get_im_itens().set_row_data(i, linha);
    }
}

fn editar_item(ctx: &AppCtx, i: i32, f: impl FnOnce(&Importacao, &mut Vinculo)) {
    let i = i as usize;
    IMPORTACAO.with(|im| {
        let mut im = im.borrow_mut();
        if i >= im.itens.len() {
            return;
        }
        let mut v = im.itens[i].clone();
        f(&im, &mut v);
        im.itens[i] = v;
    });
    atualizar_item(ctx, i);
}

/// Recalcula preço (e caixa) a partir da margem.
fn recalcular_preco(im: &Importacao, v: &mut Vinculo) {
    v.preco = preco_pela_margem(custo_efetivo(v), v.margem);
    v.preco_txt = fmt2(v.preco);
    if let Some(cx) = im.caixa_proporcional(v, v.preco) {
        v.preco_caixa = Some(cx);
        v.preco_caixa_txt = fmt2(cx);
    }
}

fn ligar_importacao(ctx: &AppCtx, store: &FiscalStore<'_>) {
    let c = ctx.clone();
    store.on_im_bip_editado(move |t| {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let digitos = so_digitos(&t);
        s.set_im_bip_digitos(digitos.len().min(44) as i32);
        s.set_im_erro(SharedString::new());
        if digitos.len() == 44 {
            carregar_nota_pela_chave(&c, digitos);
        }
    });
    let c = ctx.clone();
    store.on_im_carregar(move || {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let digitos = so_digitos(&s.get_im_bip());
        if digitos.len() != 44 {
            s.set_im_erro("A chave bipada deve ter 44 dígitos numéricos.".into());
            return;
        }
        carregar_nota_pela_chave(&c, digitos);
    });
    let c = ctx.clone();
    store.on_im_escolher_xml(move || {
        c.spawn(
            async {
                let caminho = tokio::task::spawn_blocking(|| arquivos::abrir("Arquivo XML da NF-e", &["xml"])).await.ok().flatten()?;
                let bytes = std::fs::read(&caminho).ok()?;
                Some((caminho.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "nota.xml".into()), bytes))
            },
            |ui, r| {
                if let Some((nome, bytes)) = r {
                    store_de(ui).set_im_arquivo(nome.clone().into());
                    IMPORTACAO.with(|im| im.borrow_mut().arquivo = Some((nome, bytes)));
                }
            },
        );
    });
    let c = ctx.clone();
    store.on_im_carregar_xml(move || carregar_arquivo_xml(&c));
    let c = ctx.clone();
    store.on_im_voltar_inicio(move || {
        if let Some(ui) = c.ui() {
            let s = store_de(&ui);
            s.set_im_fase("inicio".into());
            s.set_im_aviso(SharedString::new());
        }
    });
    let c = ctx.clone();
    store.on_im_confirmacao_editada(move |t| conferir_segundo_bip(&c, &t));
    let c = ctx.clone();
    store.on_im_acao(move |i, a| editar_item(&c, i, |_, v| v.acao = a));
    let c = ctx.clone();
    store.on_im_conv_ativo(move |i, ativo| {
        editar_item(&c, i, |im, v| {
            v.conv_ativo = ativo;
            v.fator = if ativo { if v.fator > 1.0 { v.fator } else { 12.0 } } else { 1.0 };
            v.fator_txt = fmt_num(v.fator);
            recalcular_preco(im, v);
        })
    });
    let c = ctx.clone();
    store.on_im_conv_preset(move |i, fator, modo| {
        editar_item(&c, i, |im, v| {
            v.fator = f64::from(fator);
            v.fator_txt = fator.to_string();
            v.conv_modo = modo;
            recalcular_preco(im, v);
        })
    });
    let c = ctx.clone();
    store.on_im_fator(move |i, t| {
        editar_item(&c, i, |im, v| {
            v.fator = t.trim().parse::<f64>().ok().filter(|f| *f >= 1.0).map(f64::floor).unwrap_or(1.0);
            v.fator_txt = t.to_string();
            recalcular_preco(im, v);
        })
    });
    let c = ctx.clone();
    store.on_im_produto(move |i, idx| {
        editar_item(&c, i, |im, v| {
            let Some(p) = im.produtos.get(idx as usize) else { return };
            v.produto = Some(idx as usize);
            v.margem = p.target_margin.filter(|m| *m > 0.0).unwrap_or(im.margem_padrao);
            v.margem_txt = fmt_num(v.margem);
            // produto vendido em caixa sugere o fator
            if let Some(q) = p.box_quantity.filter(|q| p.has_box_price && *q > 1.0) {
                v.fator = q;
                v.fator_txt = fmt_num(q);
                v.conv_ativo = true;
            }
            v.atualizar_caixa = p.has_box_price;
            v.preco_caixa = None;
            v.preco_caixa_txt.clear();
            v.manter_ncm = false;
            recalcular_preco(im, v);
        })
    });
    let c = ctx.clone();
    store.on_im_busca_abrir(move |i| {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let nome = IMPORTACAO.with(|im| im.borrow().itens.get(i as usize).map(|v| v.item.name.clone()).unwrap_or_default());
        s.set_im_busca_nota(nome.into());
        s.set_im_busca(SharedString::new());
        s.set_im_busca_item(i);
        mostrar_busca_vinculo(&ui);
    });
    let c = ctx.clone();
    store.on_im_busca_editada(move |_| {
        if let Some(ui) = c.ui() {
            mostrar_busca_vinculo(&ui);
        }
    });
    let c = ctx.clone();
    store.on_im_busca_escolher(move |idx| {
        let Some(ui) = c.ui() else { return };
        let s = store_de(&ui);
        let i = s.get_im_busca_item();
        s.set_im_busca_item(-1);
        if i >= 0 && idx >= 0 {
            s.invoke_im_produto(i, idx);
        }
    });
    let c = ctx.clone();
    store.on_im_manter_ncm(move |i, sim| editar_item(&c, i, |_, v| v.manter_ncm = sim));
    let c = ctx.clone();
    store.on_im_categoria(move |i, idx| editar_item(&c, i, |_, v| v.categoria = Some(idx as usize)));
    let c = ctx.clone();
    store.on_im_margem(move |i, t| {
        editar_item(&c, i, |im, v| {
            v.margem = num_br(&t);
            v.margem_txt = t.to_string();
            recalcular_preco(im, v);
        })
    });
    let c = ctx.clone();
    store.on_im_preco(move |i, t| {
        editar_item(&c, i, |im, v| {
            v.preco = num_br(&t);
            v.preco_txt = t.to_string();
            v.margem = margem_pelo_preco(custo_efetivo(v), v.preco);
            v.margem_txt = fmt_num(v.margem);
            if let Some(cx) = im.caixa_proporcional(v, v.preco) {
                v.preco_caixa = Some(cx);
                v.preco_caixa_txt = fmt2(cx);
            }
        })
    });
    let c = ctx.clone();
    store.on_im_atualizar_caixa(move |i, sim| editar_item(&c, i, |_, v| v.atualizar_caixa = sim));
    let c = ctx.clone();
    store.on_im_preco_caixa(move |i, t| {
        editar_item(&c, i, |_, v| {
            v.preco_caixa = Some(num_br(&t));
            v.preco_caixa_txt = t.to_string();
        })
    });
    let c = ctx.clone();
    store.on_im_aplicar(move || aplicar_importacao(&c, false));
    let c = ctx.clone();
    store.on_im_confirmar_quedas(move |sim| {
        if let Some(ui) = c.ui() {
            store_de(&ui).set_im_quedas(slint::ModelRc::default());
        }
        if sim {
            aplicar_importacao(&c, true);
        }
    });
}

fn abrir_importacao(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let chave = CHAVE_IMPORTAR.with(|k| std::mem::take(&mut *k.borrow_mut()));
    IMPORTACAO.with(|im| *im.borrow_mut() = Importacao { chave_origem: so_digitos(&chave), margem_padrao: 50.0, ..Default::default() });
    s.set_im_fase(if chave.is_empty() { "inicio" } else { "carregando" }.into());
    s.set_im_bip(SharedString::new());
    s.set_im_bip_digitos(0);
    s.set_im_erro(SharedString::new());
    s.set_im_arquivo(SharedString::new());
    s.set_im_contingencia(false);
    s.set_im_aviso(SharedString::new());
    s.set_im_confirmacao(SharedString::new());
    s.set_im_confirma_estado(0);
    s.set_im_quedas(slint::ModelRc::default());
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let produtos = barerp_db::importacao::produtos_fiscais(&db).await.unwrap_or_default();
            let categorias = barerp_db::catalog::categories(&db).await.map(|cs| cs.into_iter().map(|c| (c.id, c.name)).collect::<Vec<_>>()).unwrap_or_default();
            let margem = barerp_db::importacao::margem_padrao(&db).await.unwrap_or(50.0);
            (produtos, categorias, margem)
        },
        move |ui, (produtos, categorias, margem)| {
            let s = store_de(ui);
            let nomes: Vec<SharedString> = produtos.iter().map(|p| format!("{} (Estoque atual: {})", p.name, fmt_num(p.stock)).into()).collect();
            s.set_im_produtos(slint::ModelRc::new(slint::VecModel::from(nomes)));
            let cats: Vec<SharedString> = categorias.iter().map(|(_, n)| n.clone().into()).collect();
            s.set_im_categorias(slint::ModelRc::new(slint::VecModel::from(cats)));
            IMPORTACAO.with(|im| {
                let mut im = im.borrow_mut();
                im.produtos = produtos;
                im.categorias = categorias;
                im.margem_padrao = margem;
            });
            if !chave.is_empty() {
                carregar_nota_pela_chave(&c, so_digitos(&chave));
            }
        },
    );
}

fn itens_da_nota(d: &Value) -> Vec<ItemXmlNota> {
    d.get("items")
        .and_then(Value::as_array)
        .map(|l| {
            l.iter()
                .map(|i| ItemXmlNota {
                    code: texto(i, "code"),
                    name: texto(i, "name"),
                    quantity: numero(i, "quantity"),
                    unit_cost: numero(i, "unitCost"),
                    ncm: texto(i, "ncm"),
                    cfop: texto(i, "cfop"),
                    unit: texto(i, "unit"),
                    ean: texto(i, "ean"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Mostra a nota na conferência; `confirmada` = veio pelo bip da chave (o 2º bip já está feito).
fn abrir_conferencia(ctx: &AppCtx, d: Value, chave_confirmada: Option<String>) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    IMPORTACAO.with(|im| {
        let mut im = im.borrow_mut();
        let itens: Vec<Vinculo> = itens_da_nota(&d).into_iter().map(|it| im.vinculo_inicial(it)).collect();
        im.itens = itens;
        im.nota = d.clone();
    });
    let vendor = d.get("vendor").cloned().unwrap_or(Value::Null);
    let numero_nf = texto(&d, "numero");
    s.set_im_fornecedor(Some(texto(&vendor, "name")).filter(|n| !n.is_empty()).unwrap_or_else(|| "Fornecedor Identificado".into()).into());
    let cnpj = Some(texto(&vendor, "cnpj")).filter(|c| !c.is_empty()).unwrap_or_else(|| "N/A".into());
    let nf = if numero_nf.is_empty() { String::new() } else { format!(" • NF: {}/{}", numero_nf, Some(texto(&d, "serie")).filter(|x| !x.is_empty()).unwrap_or_else(|| "1".into())) };
    s.set_im_cnpj_nf(format!("CNPJ: {cnpj}{nf}").into());
    match chave_confirmada {
        Some(chave) => {
            s.set_im_confirmacao(chave.into());
            s.set_im_confirma_estado(1);
        }
        None => {
            s.set_im_confirmacao(SharedString::new());
            s.set_im_confirma_estado(0);
        }
    }
    s.set_im_aviso(SharedString::new());
    s.set_im_fase("conferencia".into());
    mostrar_conferencia(ctx);
}

/// Itens pela chave: XML salvo/baixado (parsed-by-chave) ou, em último caso, XML bruto + import-xml.
fn carregar_nota_pela_chave(ctx: &AppCtx, chave: String) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_im_fase("carregando".into());
    s.set_im_erro(SharedString::new());
    let server = ctx.server.clone();
    let c = ctx.clone();
    let chave2 = chave.clone();
    ctx.spawn(
        async move {
            let r = chamar(server.clone(), "GET", &format!("/parsed-by-chave/{chave}"), Corpo::Nada).await;
            if r.ok() {
                return Ok(r.json());
            }
            let bruto = chamar(server.clone(), "GET", &format!("/notas-recebidas/{chave}/xml"), Corpo::Nada).await;
            if !bruto.ok() {
                return Err(Some(bruto.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "XML não disponível na SEFAZ ainda. Aguarde alguns instantes e tente novamente.".into()));
            }
            let campo = barerp_server::CampoMultipart::arquivo("xml", &format!("nfe_{chave}.xml"), bruto.bytes);
            let lido = chamar(server, "POST", "/import-xml", Corpo::Multipart(vec![campo])).await;
            if lido.ok() { Ok(lido.json()) } else { Err(Some(lido.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao processar XML.".into())) }
        },
        move |ui, r| match r {
            Ok(d) => abrir_conferencia(&c, d, Some(chave2)),
            Err(e) => {
                let s = store_de(ui);
                s.set_im_fase("inicio".into());
                s.set_im_erro(e.into());
            }
        },
    );
}

fn carregar_arquivo_xml(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let Some((nome, bytes)) = IMPORTACAO.with(|im| im.borrow().arquivo.clone()) else { return };
    store_de(&ui).set_im_lendo(true);
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move { chamar(server, "POST", "/import-xml", Corpo::Multipart(vec![barerp_server::CampoMultipart::arquivo("xml", &nome, bytes)])).await },
        move |ui, r| {
            let s = store_de(ui);
            s.set_im_lendo(false);
            if r.ok() {
                // pelo arquivo, o 2º bip ainda precisa ser feito na conferência
                abrir_conferencia(&c, r.json(), None);
            } else {
                s.set_im_erro(r.erro().into());
            }
        },
    );
}

/// 2º bip na conferência: a chave bipada tem de ser a da nota carregada.
fn conferir_segundo_bip(ctx: &AppCtx, digitado: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let bipada = so_digitos(digitado);
    if bipada.len() < 44 {
        s.set_im_confirma_estado(0);
        return;
    }
    let chave_nota = IMPORTACAO.with(|im| {
        let im = im.borrow();
        Some(im.chave_origem.clone()).filter(|c| !c.is_empty()).unwrap_or_else(|| so_digitos(&texto(&im.nota, "accessKey")))
    });
    s.set_im_confirma_estado(if chave_nota.is_empty() || bipada == chave_nota { 1 } else { 2 });
}

fn aplicar_importacao(ctx: &AppCtx, confirmou_queda: bool) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let aviso = |m: &str| {
        s.set_im_aviso_ok(false);
        s.set_im_aviso(m.into());
    };
    let (validos, quedas, corpo) = IMPORTACAO.with(|im| {
        let im = im.borrow();
        let validos: Vec<&Vinculo> = im.itens.iter().filter(|v| v.acao != 2).collect();
        let quedas: Vec<FxQueda> = validos
            .iter()
            .filter(|v| v.acao == 0)
            .filter_map(|v| {
                let p = im.produto(v)?;
                (p.price > 0.0 && v.preco < p.price).then(|| FxQueda { nome: p.name.clone().into(), de: reais(p.price).into(), para: reais(v.preco).into() })
            })
            .collect();
        let chave = Some(so_digitos(&s.get_im_confirmacao()))
            .filter(|c| !c.is_empty())
            .or_else(|| Some(im.chave_origem.clone()).filter(|c| !c.is_empty()))
            .unwrap_or_else(|| so_digitos(&texto(&im.nota, "accessKey")));
        let vendor = im.nota.get("vendor").cloned().unwrap_or(Value::Null);
        let itens: Vec<Value> = validos
            .iter()
            .map(|v| {
                let it = &v.item;
                // NCM nulo: o vínculo mantém o NCM que o produto já tem (COALESCE no banco)
                let ncm_produto = im.produto(v).and_then(|p| p.ncm.as_deref()).map(so_digitos).unwrap_or_default();
                let ncm = if v.manter_ncm && ncm_em_conflito(v.acao, &so_digitos(&it.ncm), &ncm_produto) { None } else { Some(&it.ncm) };
                json!({
                    "xmlItem": { "name": it.name, "quantity": it.quantity, "unitCost": it.unit_cost, "ncm": ncm, "cfop": it.cfop, "ean": if it.ean.is_empty() { None } else { Some(&it.ean) }, "code": it.code, "unit": it.unit },
                    "action": if v.acao == 0 { "LINK" } else { "NEW" },
                    "productId": im.produto(v).map(|p| p.id.clone()),
                    "categoryId": v.categoria.and_then(|c| im.categorias.get(c)).map(|c| c.0.clone()),
                    "newSalePrice": v.preco,
                    "targetMargin": v.margem,
                    "updateBoxPrice": v.atualizar_caixa,
                    "newBoxPrice": v.preco_caixa,
                    "conversionFactor": if v.conv_ativo && v.fator > 0.0 { v.fator } else { 1.0 },
                })
            })
            .collect();
        let corpo = json!({
            "items": itens,
            "chaveAcesso": if chave.is_empty() { None } else { Some(chave) },
            "supplierId": vendor.get("id").filter(|v| !v.is_null()),
            "vendorName": vendor.get("name").filter(|v| !v.is_null()),
            "numero": im.nota.get("numero").filter(|v| !v.is_null() && v.as_str() != Some("")),
            "serie": im.nota.get("serie").filter(|v| !v.is_null() && v.as_str() != Some("")),
            "dataEmissao": im.nota.get("dataEmissao").filter(|v| !v.is_null()),
            "valorTotal": im.nota.get("valorTotal").filter(|v| !v.is_null()),
        });
        let validos: Vec<Vinculo> = validos.into_iter().cloned().collect();
        (validos, quedas, corpo)
    });
    if validos.is_empty() {
        return aviso("Nenhum item selecionado para importação.");
    }
    if s.get_im_confirma_estado() == 2 {
        return aviso("Atenção: A chave de acesso bipada na conferência não confere com o DANFE desta nota. Verifique o código de barras antes de efetivar.");
    }
    if validos.iter().any(|v| v.acao == 1 && v.categoria.is_none()) {
        return aviso("Selecione uma categoria para os novos produtos.");
    }
    if validos.iter().any(|v| v.acao == 0 && v.produto.is_none()) {
        return aviso("Selecione qual produto existente deseja vincular.");
    }
    if !confirmou_queda && !quedas.is_empty() {
        s.set_im_quedas(slint::ModelRc::new(slint::VecModel::from(quedas)));
        return;
    }
    s.set_im_aplicando(true);
    let usuario = ctx.user_name();
    let db = ctx.db.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let imp: barerp_db::importacao::ImportacaoXml = serde_json::from_value(corpo).map_err(|e| e.to_string())?;
            barerp_db::importacao::aplicar_importacao_xml(&db, &imp, usuario.as_deref()).await.map_err(|e| e.to_string())
        },
        move |ui, r| {
            let s = store_de(ui);
            s.set_im_aplicando(false);
            match r {
                Ok(res) => {
                    c.status(
                        format!("✅ Entrada no Estoque Confirmada com Sucesso! Produtos atualizados: {} • Novos produtos cadastrados: {}", res.updated, res.created),
                        false,
                    );
                    abrir(&c, "hub");
                }
                Err(e) => {
                    s.set_im_aviso_ok(false);
                    s.set_im_aviso(format!("Erro ao efetivar entrada: {e}").into());
                }
            }
        },
    );
}

// ============================================================
// 1º BIP: Recebimento (NfReceivingView)
// ============================================================

/// "1526 0936 2751 6300 0124 5…" (primeiros 24 caracteres da chave em grupos de 4)
fn chave_curta(chave: &str) -> String {
    let grupos: String = chave.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ");
    format!("{}…", grupos.chars().take(24).collect::<String>())
}

fn recebida_da_lista(n: &Value) -> FxRecebida {
    let status = texto(n, "status");
    let (estado, rotulo) = match status.as_str() {
        "finalizada" | "importada" => (2, "✓ 2º Bip OK (No Estoque)"),
        "ciencia_registrada" => (1, "Ciência SEFAZ OK (Processando XML)"),
        _ => (0, "Aguardando 2º Bip (Conferência)"),
    };
    let valor = numero(n, "valorTotal");
    let data = chrono::DateTime::parse_from_rfc3339(&texto(n, "createdAt"))
        .map(|d| d.with_timezone(&chrono::Local).format("%d/%m/%Y").to_string())
        .unwrap_or_default();
    FxRecebida {
        chave: texto(n, "chave").into(),
        chave_curta: chave_curta(&texto(n, "chave")).into(),
        emitente: Some(texto(n, "emitente")).filter(|e| !e.is_empty()).unwrap_or_else(|| "Fornecedor não identificado".into()).into(),
        nf: format!("{}/{}", texto(n, "numero"), texto(n, "serie")).into(),
        estado,
        rotulo: rotulo.into(),
        valor: if estado == 1 && valor <= 0.0 { "Aguardando SEFAZ...".into() } else { reais(valor).into() },
        data: data.into(),
    }
}

fn carregar_recebidas(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    store_de(&ui).set_rb_carregando(true);
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", "/notas-recebidas", Corpo::Nada).await.json() }, |ui, d| {
        let s = store_de(ui);
        let lista: Vec<FxRecebida> = d.as_array().map(|l| l.iter().map(recebida_da_lista).collect()).unwrap_or_default();
        s.set_rb_notas(slint::ModelRc::new(slint::VecModel::from(lista)));
        s.set_rb_carregando(false);
    });
}

fn bipar(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let chave = so_digitos(&s.get_rb_chave());
    if chave.len() != 44 {
        s.set_rb_resultado(FxBip {
            visivel: true,
            erro: "Chave de acesso inválida. Bipe novamente ou verifique os 44 dígitos.".into(),
            ..Default::default()
        });
        return;
    }
    s.set_rb_bipando(true);
    s.set_rb_resultado(FxBip::default());
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move { chamar(server, "POST", "/bip-chave", Corpo::Json(json!({ "chave": chave }))).await },
        move |ui, r| {
            let s = store_de(ui);
            s.set_rb_bipando(false);
            s.set_rb_chave(SharedString::new());
            let d = r.json();
            s.set_rb_resultado(if r.ok() {
                FxBip {
                    visivel: true,
                    ok: true,
                    xml: d.get("xmlDisponivel").and_then(Value::as_bool).unwrap_or(false),
                    erro: SharedString::new(),
                    emitente: texto(&d, "emitente").into(),
                    nf: format!("{}/{}", texto(&d, "numero"), texto(&d, "serie")).into(),
                    valor: reais(numero(&d, "valorTotal")).into(),
                    chave: texto(&d, "chave").into(),
                }
            } else {
                FxBip { visivel: true, erro: r.erro().into(), ..Default::default() }
            });
            carregar_recebidas(&c);
        },
    );
}

fn buscar_xml_recebida(ctx: &AppCtx, chave: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_rb_sincronizando(chave.into());
    s.set_rb_aviso(SharedString::new());
    let (server, caminho) = (ctx.server.clone(), format!("/notas-recebidas/{}/sync", so_digitos(chave)));
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", &caminho, Corpo::Nada).await }, move |ui, r| {
        let s = store_de(ui);
        s.set_rb_sincronizando(SharedString::new());
        let d = r.json();
        let (ok, msg) = if !r.ok() {
            (false, format!("Erro ao buscar na SEFAZ: {}", r.erro()))
        } else if d.get("xmlDisponivel").and_then(Value::as_bool) == Some(true) {
            (true, Some(texto(&d, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "XML sincronizado com sucesso!".into()))
        } else {
            (false, Some(texto(&d, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "A SEFAZ ainda não liberou o arquivo XML desta nota.".into()))
        };
        s.set_rb_aviso_ok(ok);
        s.set_rb_aviso(msg.into());
        carregar_recebidas(&c);
    });
}

// ============================================================
// Notas Pendentes (PendingInvoicesView)
// ============================================================

fn chave_formatada(c: &str) -> String {
    c.as_bytes().chunks(4).map(|g| String::from_utf8_lossy(g).into_owned()).collect::<Vec<_>>().join(" ")
}

fn documento_fmt(d: &str) -> String {
    let d = so_digitos(d);
    match d.len() {
        14 => barerp_print::format::cnpj(&d),
        11 => format!("{}.{}.{}-{}", &d[0..3], &d[3..6], &d[6..9], &d[9..11]),
        _ => d,
    }
}

/// `toLocaleDateString('pt-BR', { dia, mês, ano, hora, minuto })`; datas sem hora ("2026-09-11") ficam só com o dia.
fn data_hora_curta(iso: &str) -> String {
    if iso.is_empty() {
        return "-".into();
    }
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(iso) {
        return d.with_timezone(&chrono::Local).format("%d/%m/%Y, %H:%M").to_string();
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(iso.get(..10).unwrap_or(iso), "%Y-%m-%d") {
        return d.format("%d/%m/%Y, 00:00").to_string();
    }
    iso.to_string()
}

fn pendente_da_lista(n: &Value) -> FxPendente {
    let estado = match texto(n, "status").as_str() {
        "finalizada" => 2,
        "recebida" => 1,
        _ => 0,
    };
    let numero_nf = Some(texto(n, "numero")).filter(|v| !v.is_empty()).unwrap_or_else(|| "S/N".into());
    let serie = Some(texto(n, "serie")).filter(|v| !v.is_empty()).unwrap_or_else(|| "1".into());
    let cnpj = texto(n, "cnpjEmitente");
    FxPendente {
        chave: texto(n, "chave").into(),
        chave_fmt: chave_formatada(&texto(n, "chave")).into(),
        emitente: Some(texto(n, "emitente")).filter(|v| !v.is_empty()).unwrap_or_else(|| "Fornecedor".into()).into(),
        cnpj: if cnpj.is_empty() { SharedString::new() } else { documento_fmt(&cnpj).into() },
        nf: format!("NF {numero_nf} • Série {serie}").into(),
        data: data_hora_curta(&texto(n, "dataEmissao")).into(),
        valor: reais(numero(n, "valorTotal")).into(),
        estado,
        xml: n.get("hasXml").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn carregar_pendentes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mut q: Vec<String> = Vec::new();
    let mut add = |k: &str, v: String| {
        if !v.trim().is_empty() && v != "todos" {
            q.push(format!("{k}={}", uri_seg(v.trim())));
        }
    };
    add("status", s.get_pd_status().to_string());
    add("xmlStatus", s.get_pd_xml().to_string());
    add("dataInicio", s.get_pd_inicio().to_string());
    add("dataFim", s.get_pd_fim().to_string());
    add("fornecedor", s.get_pd_busca().to_string());
    s.set_pd_carregando(true);
    let caminho = format!("/notas-pendentes?{}", q.join("&"));
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", &caminho, Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_pd_carregando(false);
        if !r.ok() {
            return;
        }
        let d = r.json();
        let lista: Vec<FxPendente> = d.get("notas").and_then(Value::as_array).map(|l| l.iter().map(pendente_da_lista).collect()).unwrap_or_default();
        s.set_pd_notas(slint::ModelRc::new(slint::VecModel::from(lista)));
        if let Some(r) = d.get("summary") {
            let n = |k: &str| numero(r, k) as i32;
            s.set_pd_total(n("total"));
            s.set_pd_pendentes(n("pendentes"));
            s.set_pd_recebidas(n("recebidas"));
            s.set_pd_finalizadas(n("finalizadas"));
            s.set_pd_com_xml(n("comXml"));
            s.set_pd_sem_xml(n("semXml"));
        }
    });
}

fn periodo_rapido(ctx: &AppCtx, tipo: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let hoje = chrono::Local::now().date_naive();
    let inicio = match tipo {
        "hoje" => hoje,
        "7d" => hoje - chrono::Duration::days(7),
        "30d" => hoje - chrono::Duration::days(30),
        "mes" => hoje.with_day(1).unwrap_or(hoje),
        _ => return,
    };
    s.set_pd_inicio(inicio.format("%Y-%m-%d").to_string().into());
    s.set_pd_fim(hoje.format("%Y-%m-%d").to_string().into());
    carregar_pendentes(ctx);
}

fn aviso_pendentes(ctx: &AppCtx, ok: bool, msg: String) {
    if let Some(ui) = ctx.ui() {
        let s = store_de(&ui);
        s.set_pd_aviso_ok(ok);
        s.set_pd_aviso(msg.into());
    }
}

fn sincronizar_pendentes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_pd_sincronizando(true);
    s.set_pd_aviso(SharedString::new());
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "GET", "/sync-nfe-recebidas", Corpo::Nada).await }, move |ui, r| {
        store_de(ui).set_pd_sincronizando(false);
        if r.ok() {
            let msg = Some(texto(&r.json(), "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Sincronização concluída com sucesso!".into());
            aviso_pendentes(&c, true, msg);
        } else {
            aviso_pendentes(&c, false, Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro de conexão ao consultar a SEFAZ.".into()));
        }
        carregar_pendentes(&c);
        // o aviso some sozinho, como o toast do original
        let c2 = c.clone();
        Timer::single_shot(std::time::Duration::from_secs(7), move || {
            if let Some(ui) = c2.ui() {
                store_de(&ui).set_pd_aviso(SharedString::new());
            }
        });
    });
}

fn pedir_confirmacao(ctx: &AppCtx, acao: &str, chave: &str) {
    let Some(ui) = ctx.ui() else { return };
    let (titulo, texto_conf) = match acao {
        "receber" => ("Registrar 1º BIP", "Deseja registrar o 1º BIP para esta nota? O sistema registrará a Ciência da Operação na SEFAZ e atualizará o status para Recebida."),
        "finalizar" => ("Marcar como finalizada", "Deseja marcar esta nota fiscal como FINALIZADA?\n\nFaça isso se você já deu entrada física ou anterior nas mercadorias desta nota no estoque."),
        "reabrir" => ("Reabrir nota", "Deseja reabrir esta nota para conferência (2º Bip)?"),
        _ => return,
    };
    store_de(&ui).set_pd_confirma(FxConfirma {
        visivel: true,
        titulo: titulo.into(),
        texto: texto_conf.into(),
        acao: acao.into(),
        chave: chave.into(),
    });
}

fn confirmar_acao_pendente(ctx: &AppCtx, sim: bool) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let conf = s.get_pd_confirma();
    s.set_pd_confirma(FxConfirma::default());
    if !sim {
        return;
    }
    let (acao, chave) = (conf.acao.to_string(), so_digitos(&conf.chave));
    s.set_pd_acao_chave(chave.clone().into());
    let caminho = format!("/notas-pendentes/{chave}/{acao}");
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", &caminho, Corpo::Nada).await }, move |ui, r| {
        store_de(ui).set_pd_acao_chave(SharedString::new());
        let verbo = match acao.as_str() {
            "receber" => "receber",
            "finalizar" => "finalizar",
            _ => "reabrir",
        };
        if !r.ok() {
            aviso_pendentes(&c, false, format!("Falha ao {verbo} nota: {}", r.erro()));
        } else if acao == "receber" {
            aviso_pendentes(&c, true, Some(texto(&r.json(), "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Nota recebida com sucesso!".into()));
        }
        carregar_pendentes(&c);
    });
}

fn buscar_xml_pendente(ctx: &AppCtx, chave: &str) {
    let Some(ui) = ctx.ui() else { return };
    let chave = so_digitos(chave);
    store_de(&ui).set_pd_acao_chave(chave.clone().into());
    let caminho = format!("/notas-recebidas/{chave}/sync");
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(async move { chamar(server, "POST", &caminho, Corpo::Nada).await }, move |ui, r| {
        store_de(ui).set_pd_acao_chave(SharedString::new());
        if r.ok() {
            let d = r.json();
            let ok = d.get("xmlDisponivel").and_then(Value::as_bool).unwrap_or(false);
            aviso_pendentes(&c, ok, Some(texto(&d, "message")).filter(|m| !m.is_empty()).unwrap_or_else(|| "Consulta SEFAZ finalizada.".into()));
        } else {
            aviso_pendentes(&c, false, format!("Falha ao buscar XML: {}", r.erro()));
        }
        carregar_pendentes(&c);
    });
}

fn baixar_xml_pendente(ctx: &AppCtx, chave: &str) {
    let chave = so_digitos(chave);
    let server = ctx.server.clone();
    let c = ctx.clone();
    ctx.spawn(
        async move {
            let r = chamar(server, "GET", &format!("/notas-recebidas/{chave}/xml"), Corpo::Nada).await;
            if !r.ok() {
                return Err(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "XML não disponível no momento.".into()));
            }
            let nome = format!("NFe_{chave}.xml");
            let destino = tokio::task::spawn_blocking(move || arquivos::salvar("Salvar XML da NF-e", &nome)).await.ok().flatten();
            let Some(destino) = destino else { return Ok(false) };
            std::fs::write(&destino, &r.bytes).map_err(|e| e.to_string())?;
            Ok(true)
        },
        move |_, r| match r {
            Ok(true) => aviso_pendentes(&c, true, "XML salvo.".into()),
            Ok(false) => {}
            Err(e) => aviso_pendentes(&c, false, format!("Erro ao baixar XML: {e}")),
        },
    );
}

// ============================================================
// Painel do Contador (AccountantPanelView)
// ============================================================

fn ano_mes(t: &str) -> Option<(i32, u32)> {
    let (a, m) = t.trim().split_once('-')?;
    let (a, m) = (a.parse::<i32>().ok()?, m.parse::<u32>().ok()?);
    (1..=12).contains(&m).then_some((a, m))
}

const MESES: [&str; 12] =
    ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];

fn mudar_mes(ctx: &AppCtx, mes: &str) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let Some((a, m)) = ano_mes(mes) else { return };
    let mes = format!("{a:04}-{m:02}");
    s.set_ct_mes(mes.clone().into());
    s.set_ct_mes_rotulo(format!("{} de {a}", MESES[(m - 1) as usize]).into());
    s.set_ct_carregando(true);
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", &format!("/month-summary?month={mes}"), Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_ct_carregando(false);
        if !r.ok() {
            s.set_ct_tem_resumo(false);
            return;
        }
        let d = r.json();
        let qtd = |k: &str| SharedString::from((numero(&d, k) as i64).to_string());
        s.set_ct_tem_resumo(true);
        s.set_ct_nfce_qtd(qtd("nfceCount"));
        s.set_ct_nfce_total(reais(numero(&d, "nfceTotal")).into());
        s.set_ct_nfe_qtd(qtd("nfeCount"));
        s.set_ct_nfe_total(reais(numero(&d, "nfeTotal")).into());
        s.set_ct_ent_qtd(qtd("recebidasCount"));
        s.set_ct_ent_total(reais(numero(&d, "recebidasTotal")).into());
        s.set_ct_tem_notas(d.get("hasNotes").and_then(Value::as_bool).unwrap_or(false));
        s.set_ct_total_notas((numero(&d, "emitidasCount") + numero(&d, "recebidasCount")) as i32);
        s.set_ct_tem_estoque(d.get("stockCostTotal").is_some());
        s.set_ct_estoque_custo(reais(numero(&d, "stockCostTotal")).into());
        s.set_ct_estoque_venda(reais(numero(&d, "stockSaleTotal")).into());
        s.set_ct_estoque_lucro(reais(numero(&d, "stockProfitTotal")).into());
        let cats: Vec<FxCategoria> = d
            .get("categoriesStock")
            .and_then(Value::as_array)
            .map(|cs| {
                cs.iter()
                    .map(|c| FxCategoria {
                        nome: texto(c, "category").into(),
                        itens: numero(c, "itemsCount") as i32,
                        custo: reais(numero(c, "totalCost")).into(),
                        lucro: reais(numero(c, "expectedProfit")).into(),
                        margem: texto(c, "marginPercent").into(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        s.set_ct_categorias(slint::ModelRc::new(slint::VecModel::from(cats)));
    });
}

/// Gera o .ZIP do mês e pergunta onde salvar (o navegador baixava direto para Downloads).
fn exportar_mes(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    let mes = s.get_ct_mes().to_string();
    let incluir = s.get_ct_incluir_estoque();
    s.set_ct_exportando(true);
    s.set_ct_mensagem(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(
        async move {
            let r = chamar(server, "GET", &format!("/export-month?month={mes}&includeStock={incluir}"), Corpo::Nada).await;
            if !r.ok() {
                return Err(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao gerar o pacote ZIP.".into()));
            }
            let nome = format!("Fechamento_Contabil_{mes}.zip");
            let destino = tokio::task::spawn_blocking(move || arquivos::salvar("Salvar fechamento contábil", &nome))
                .await
                .ok()
                .flatten();
            let Some(destino) = destino else { return Ok(None) };
            std::fs::write(&destino, &r.bytes).map_err(|e| format!("Não foi possível salvar o arquivo: {e}"))?;
            Ok(Some(destino))
        },
        |ui, r| {
            let s = store_de(ui);
            s.set_ct_exportando(false);
            match r {
                Ok(Some(_)) => {
                    s.set_ct_mensagem_ok(true);
                    s.set_ct_mensagem("Pacote baixado com sucesso! Envie o arquivo ZIP gerado para a sua contabilidade.".into());
                }
                Ok(None) => {}
                Err(e) => {
                    s.set_ct_mensagem_ok(false);
                    s.set_ct_mensagem(e.into());
                }
            }
        },
    );
}

fn carregar_backups(ctx: &AppCtx) {
    let Some(ui) = ctx.ui() else { return };
    let s = store_de(&ui);
    s.set_ct_bk_carregando(true);
    s.set_ct_bk_erro(SharedString::new());
    let server = ctx.server.clone();
    ctx.spawn(async move { chamar(server, "GET", "/focus-backups", Corpo::Nada).await }, |ui, r| {
        let s = store_de(ui);
        s.set_ct_bk_carregando(false);
        let d = r.json();
        if !r.ok() {
            s.set_ct_bk_erro(Some(r.erro()).filter(|e| !e.is_empty()).unwrap_or_else(|| "Erro ao carregar backups da Focus NFe.".into()).into());
            s.set_ct_backups(slint::ModelRc::default());
            return;
        }
        let lista = d.get("backups").and_then(Value::as_array).cloned().unwrap_or_default();
        if lista.is_empty() {
            let msg = texto(&d, "mensagem");
            if !msg.is_empty() {
                s.set_ct_bk_erro(msg.into());
            }
        }
        let itens: Vec<FxBackup> = lista
            .iter()
            .map(|b| {
                let mes = texto(b, "mes");
                let mes2 = if mes.is_empty() { String::new() } else { format!("{:0>2}", mes) };
                let rotulo = Some(texto(b, "referencia")).filter(|r| !r.is_empty()).unwrap_or_else(|| format!("{}-{mes2}", texto(b, "ano")));
                FxBackup {
                    rotulo: rotulo.into(),
                    mes: if mes2.is_empty() { "NF".into() } else { mes2.into() },
                    xmls: texto(b, "xmls").into(),
                    danfes: texto(b, "danfes").into(),
                }
            })
            .collect();
        s.set_ct_backups(slint::ModelRc::new(slint::VecModel::from(itens)));
    });
}

// ============================================================
// NFC-e da venda de balcão (usado pelo Caixa & PDV)
// ============================================================

/// Item vendido, com o preço que o caixa cobrou (promoção do dia e adicionais já aplicados).
#[derive(Debug, Clone, Default)]
pub struct ItemVendaNfce {
    pub product_id: String,
    /// Na venda por caixa (BOX), o EAN da caixa se houver.
    pub ean: String,
    pub codigo: String,
    pub quantidade: f64,
    pub preco_unitario: f64,
    /// Total cobrado na linha (base do rateio do desconto).
    pub total: f64,
    /// Nome com os adicionais: "Caipirinha (Limão, Gelo)".
    pub nome: String,
    pub ncm: String,
    pub cfop: String,
    /// "CX" na venda por caixa.
    pub unidade: String,
    pub cest: String,
}

/// Venda já gravada que vai virar NFC-e (`emitNfceForOrder`).
#[derive(Debug, Clone, Default)]
pub struct VendaNfce {
    /// Id da venda: referência fixa na Focus, então "tentar de novo" nunca gera uma segunda nota.
    pub sale_key: String,
    pub numero_pedido: i64,
    pub documento: String,
    pub nome_cliente: String,
    /// Forma de pagamento do caixa (CASH, CREDIT_CARD, DEBIT_CARD, PIX, CREDIT_TAB).
    pub forma_pagamento: String,
    /// Desconto da venda, rateado nos itens para o total da nota bater com o valor pago.
    pub desconto: f64,
    pub itens: Vec<ItemVendaNfce>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResultadoNfce {
    Autorizada { referencia: String },
    /// Na fila da SEFAZ: não emitir de novo, consultar depois na Reimpressão.
    Processando { referencia: String },
    Erro(String),
}

/// Corpo do `/emit-nfce` igual ao do `client/src/services/nfce.ts`.
pub fn corpo_nfce_da_venda(v: &VendaNfce) -> Value {
    let totais: Vec<f64> = v.itens.iter().map(|i| i.total).collect();
    let descontos = ratear_desconto(&totais, v.desconto);
    let itens: Vec<Value> = v
        .itens
        .iter()
        .zip(descontos)
        .map(|(i, d)| {
            json!({
                "productId": i.product_id,
                "ean": i.ean,
                "code": i.codigo,
                "quantity": i.quantidade,
                "price": i.preco_unitario,
                "discount": if d > 0.0 { Some(d) } else { None },
                "name": i.nome,
                "ncm": i.ncm.chars().filter(char::is_ascii_digit).collect::<String>(),
                "cfop": if i.cfop.is_empty() { "5102" } else { &i.cfop },
                "unit": if i.unidade.is_empty() { "un" } else { &i.unidade },
                "cest": if i.cest.is_empty() { None } else { Some(&i.cest) },
            })
        })
        .collect();
    let nome = v.nome_cliente.trim();
    json!({
        "saleKey": v.sale_key,
        "orderId": format!("bal{}", v.numero_pedido),
        "customerCpf": if v.documento.trim().is_empty() { None } else { Some(v.documento.trim()) },
        "customerName": if nome.is_empty() || nome == "Cliente Balcão" { None } else { Some(nome) },
        "paymentMethod": v.forma_pagamento,
        "items": itens,
    })
}

fn resultado_emissao(r: &RespostaFiscal) -> ResultadoNfce {
    let d = r.json();
    let referencia = d.get("referencia").or_else(|| d.get("ref")).and_then(Value::as_str).unwrap_or("").to_string();
    match (r.ok(), d.get("status").and_then(Value::as_str)) {
        (true, Some("autorizado")) => ResultadoNfce::Autorizada { referencia },
        (true, _) => ResultadoNfce::Processando { referencia },
        (false, _) => ResultadoNfce::Erro(r.erro()),
    }
}

/// Emite a NFC-e de uma venda gravada em segundo plano e entrega o resultado na thread da interface.
pub fn emitir_nfce_da_venda(ctx: &AppCtx, venda: VendaNfce, pronto: impl FnOnce(&AppWindow, ResultadoNfce) + Send + 'static) {
    let server = ctx.server.clone();
    let corpo = corpo_nfce_da_venda(&venda);
    ctx.spawn(
        async move { resultado_emissao(&chamar(server, "POST", "/emit-nfce", Corpo::Json(corpo)).await) },
        pronto,
    );
}

// ============================================================
// Impressão do DANFE NFC-e na bobina do caixa
// ============================================================

async fn imprimir_danfe(db: barerp_db::Db, server: Option<ServerHandle>, referencia: String) -> Result<String, String> {
    let caminho = format!("/xml-emitida/{}", uri_seg(&referencia));
    let r = chamar(server, "GET", &caminho, Corpo::Nada).await;
    if !r.ok() {
        return Err(r.erro());
    }
    let xml = String::from_utf8_lossy(&r.bytes).into_owned();
    let danfe = ler_danfe_nfce(&xml).map_err(|e| e.to_string())?;
    let ajustes = printer::get_printer_settings(&db).await.map_err(|e| format!("Erro ao ler a impressora: {e}"))?;
    let Some(destino) = ajustes.cashier_printer.clone().filter(|p| !p.trim().is_empty()) else {
        return Err("Nenhuma impressora de caixa configurada em Configurações > Impressoras.".into());
    };
    let (papel, modulo) = if ajustes.paper_width <= 58.0 { (PaperWidth::Mm58, 4) } else { (PaperWidth::Mm80, 5) };
    let mut doc = EscPos::new(papel, Codepage::Pc850);
    danfe::desenhar(&mut doc, &danfe, modulo);
    if ajustes.extra_feed_lines > 0 {
        doc.feed(ajustes.extra_feed_lines.clamp(0, 255) as u8);
    }
    if ajustes.auto_cut {
        doc.cut(0);
    }
    let bytes = doc.to_bytes();
    let trabalho = format!("NFC-e {}", danfe.numero);
    tokio::task::spawn_blocking(move || crate::print::dispatch_print(&destino, &trabalho, &bytes))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Falha na impressão: {e}"))?;
    Ok(format!("DANFE da NFC-e nº {} impresso.", danfe.numero))
}

/// Imprime o DANFE NFC-e da nota (referência ou chave) na impressora do caixa; o resultado vai para o rodapé.
pub fn imprimir_danfe_nfce(ctx: &AppCtx, referencia: &str) {
    let (db, server, referencia) = (ctx.db.clone(), ctx.server.clone(), referencia.to_string());
    let ctx2 = ctx.clone();
    ctx.spawn(imprimir_danfe(db, server, referencia), move |_, r| match r {
        Ok(msg) => ctx2.status(msg, false),
        Err(e) => ctx2.status(format!("DANFE não impresso: {e}"), true),
    });
}

/// `encodeURIComponent`.
fn uri_seg(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpo_da_venda_igual_ao_do_client() {
        let item = |nome: &str, total: f64| ItemVendaNfce {
            product_id: format!("p-{nome}"),
            codigo: "5001".into(),
            quantidade: 1.0,
            preco_unitario: total,
            total,
            nome: nome.into(),
            ncm: "2203.00.00".into(),
            ..Default::default()
        };
        let v = VendaNfce {
            sale_key: "ckvenda".into(),
            numero_pedido: 7,
            documento: " ".into(),
            nome_cliente: "Cliente Balcão".into(),
            forma_pagamento: "PIX".into(),
            desconto: 10.0,
            itens: vec![item("A", 10.0), item("B", 10.0), item("C", 10.0)],
        };
        let c = corpo_nfce_da_venda(&v);
        assert_eq!(c["saleKey"], "ckvenda");
        assert_eq!(c["orderId"], "bal7");
        assert!(c["customerCpf"].is_null() && c["customerName"].is_null());
        let d: Vec<f64> = c["items"].as_array().unwrap().iter().map(|i| i["discount"].as_f64().unwrap()).collect();
        assert_eq!(d, vec![3.33, 3.33, 3.34]);
        assert_eq!((c["items"][0]["ncm"].as_str(), c["items"][0]["cfop"].as_str(), c["items"][0]["unit"].as_str()), (Some("22030000"), Some("5102"), Some("un")));
    }

    #[test]
    fn resultado() {
        let r = |status: u16, corpo: Value| RespostaFiscal { status, content_type: "application/json".into(), bytes: corpo.to_string().into_bytes() };
        assert_eq!(resultado_emissao(&r(200, json!({ "status": "autorizado", "referencia": "nfce_x" }))), ResultadoNfce::Autorizada { referencia: "nfce_x".into() });
        assert_eq!(resultado_emissao(&r(200, json!({ "status": "processando", "ref": "nfce_y" }))), ResultadoNfce::Processando { referencia: "nfce_y".into() });
        assert_eq!(resultado_emissao(&r(400, json!({ "error": "SEFAZ recusou" }))), ResultadoNfce::Erro("SEFAZ recusou".into()));
    }

    #[test]
    fn ncm_da_nota_no_vinculo() {
        assert_eq!(ncm_formatado("22030000"), "2203.00.00");
        assert_eq!(ncm_formatado("2203"), "2203");
        assert_eq!(ncm_formatado(""), "");
        // vincular com NCM diferente e válido no cadastro: pergunta
        assert!(ncm_em_conflito(0, "22030000", "22021000"));
        // mesmo NCM, cadastro sem NCM (ou inválido), nota sem NCM ou item criado: grava o da nota sem perguntar
        assert!(!ncm_em_conflito(0, "22030000", "22030000"));
        assert!(!ncm_em_conflito(0, "22030000", ""));
        assert!(!ncm_em_conflito(0, "22030000", "2203"));
        assert!(!ncm_em_conflito(0, "", "22021000"));
        assert!(!ncm_em_conflito(1, "22030000", "22021000"));
    }

    #[test]
    fn busca_de_produto_para_vincular() {
        use barerp_db::importacao::ProdutoFiscal;
        let p = |nome: &str, codigo: &str, ean: &str| ProdutoFiscal {
            name: nome.into(),
            code: Some(codigo.into()),
            ean: Some(ean.into()).filter(|e: &String| !e.is_empty()),
            ..Default::default()
        };
        let im = Importacao {
            // em ordem alfabética, como vem do banco
            produtos: vec![
                p("Água Mineral 500ml", "101", ""),
                p("Cerveja Heineken Lata 350ml", "201", "7896045506873"),
                p("Cerveja Original 600ml", "202", ""),
                p("Porção de Fritas", "301", ""),
            ],
            itens: vec![Vinculo { item: ItemXmlNota { name: "CERV HEINEKEN LATA 350ML CX12".into(), ..Default::default() }, ..Default::default() }],
            ..Default::default()
        };
        let nomes = |r: Vec<FxProdutoAchado>| r.iter().map(|a| a.nome.to_string()).collect::<Vec<_>>();
        // sem texto: o mais parecido com o item da nota vem primeiro e marcado; os que só têm "CERV" em comum vêm
        // logo depois, sem a marca; o resto em ordem alfabética
        let r = im.busca_vinculo(0, "");
        assert_eq!(r.len(), 4);
        assert_eq!((r[0].nome.as_str(), r[0].sugerido, r[0].idx), ("Cerveja Heineken Lata 350ml", true, 1));
        assert_eq!((r[1].nome.as_str(), r[1].sugerido), ("Cerveja Original 600ml", false));
        assert_eq!(nomes(r)[2..], ["Água Mineral 500ml", "Porção de Fritas"]);
        // palavras em qualquer ordem, sem acento e sem maiúscula
        assert_eq!(nomes(im.busca_vinculo(0, "agua")), vec!["Água Mineral 500ml"]);
        assert_eq!(nomes(im.busca_vinculo(0, "600 cerveja")), vec!["Cerveja Original 600ml"]);
        // código de barras e código interno
        assert_eq!(nomes(im.busca_vinculo(0, "7896045506873")), vec!["Cerveja Heineken Lata 350ml"]);
        assert_eq!(nomes(im.busca_vinculo(0, "301")), vec!["Porção de Fritas"]);
        assert!(im.busca_vinculo(0, "vodka").is_empty());
    }
}
