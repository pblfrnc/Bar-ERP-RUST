# Paridade — Mesas & Salão (+ shell do app)

Arquivos: `ui/pages/mesas.slint`, `ui/pages/mesas_modals.slint`, `src/pages/mesas.rs`, `src/pages/mesas/{launch,checkout}.rs`.
Prints: `cargo run -p barerp-app --example mesas_shots -- <cópia do banco> <pasta>` (temas escuro e claro).

Legenda: `[x]` feito · `[ ]` falta · `[~]` diferente de propósito (motivo ao lado)

## Shell do app (App.tsx, Navbar.tsx, LoginScreen.tsx)
- [x] Abas do menu iguais ao original: Mesas & Salão, KDS (Cozinha), Caixa & PDV, Cardápio & Estoque, Módulo Fiscal, Retaguarda
- [x] Abas visíveis conforme as permissões do usuário (`permissions`: tables, kds, cash, products, fiscal, settings…); entra na primeira aba permitida
- [x] Indicador Online/Offline (servidor dos celulares; o motivo da falha vai para o rodapé)
- [x] Nome do operador logado (clique troca a senha) e botão sair
- [x] Botão "Nova Versão" pulsante quando há atualização (abre a janela de atualização, ver `retaguarda.md`)
- [x] Aviso de mensalidade vencendo (≤ 5 dias) com dados de PIX (`LicenseInfoModal`, área Retaguarda)
- [x] Tela cheia (F11 / botão)
- [x] Tema claro e escuro
- [x] Escala de fonte Normal (100%) / Grande (125%) / Extra Grande (140%), lembrada entre aberturas
- [x] Conectar celular (QR Code com IP da rede) — ConnectMobileModal, com ajuda do Firewall (pela Retaguarda e pelo KDS)
- [x] Login: escolher funcionário + senha/PIN, bloqueio após 5 tentativas (15 s)
- [~] Login: senha mestra 9999 removida (decisão do Pablo, 18/10/2026); admin inicial 1234 com troca sugerida
- [x] Trocar a própria senha
- [x] Bloqueio estrito por licença expirada/bloqueada (tela de ativação) — LicenseModal (área Retaguarda)
- [x] Atalhos do PC: Ctrl+A abrir caixa, F2 balcão, Ctrl+S sangria, Ctrl+Shift+S suprimento, Ctrl+F fechar turno (área Caixa), F3 KDS

## Mapa de mesas (TablesView.tsx, TableCard.tsx)
- [x] Indicadores no topo: Ocupação X/Y (%), Livres N mesas, Fechando N contas, Em Aberto R$
- [x] Busca "Buscar mesa ou cliente (ex: 2, Mariana)" por número, nome da mesa e nome do cliente
- [x] Botões: Atualizar, Comandas Finalizadas, Editar Locais, Nova Mesa
- [x] Filtro por área ("Todas as Áreas" + seções)
- [x] Filtro por status com contagem: Todas (N), Livres (N), Ocupadas (N), Fechando (N)
- [x] Estado vazio "Nenhuma mesa encontrada"
- [x] Cartão: número grande, nome, seção, badge Livre/Ocupada/Fechamento (fechamento pulsando)
- [x] Cartão livre: "Capacidade: N lugares" + botão "Abrir Mesa"
- [x] Cartão ocupado: cliente (Np), tempo aberto, "Consumo (N itens)" R$, Haver/Falta, selo +10%
- [x] Cartão ocupado: botões "Pedido" (lançar direto) e "Conta"/"Receber" (fechar direto)
- [~] Nova Mesa: número*, nome, capacidade, ambiente (Salão Principal, Deck Externo, Balcão, Área VIP) — capacidade por atalhos 2–12 em vez de campo numérico livre (toque)

## Abrir mesa (OpenTableModal.tsx)
- [x] Quantidade de pessoas com atalhos 1,2,3,4,5,6,8,10 (padrão = capacidade da mesa)
- [x] Nome do cliente ("Ex: Carlos, Família Souza...")
- [x] Garçom: selecionar da lista de garçons ou "Digitar Outro"; grava waiterId/waiterName
- [x] Abre a comanda e ocupa a mesa

## Detalhe da mesa (TableDetailsModal.tsx)
- [x] Cabeçalho: nome/Mesa N, "CMD #", selo Em Fechamento, cliente (Np), garçom, tempo
- [x] Ações: Lançar Pedido, Transferir Mesa, Juntar Mesa, Pedir Pré-Conta / Reabrir Mesa, Perda (Calote), Imprimir
- [x] Perda (Calote): confirmação, fecha a comanda, libera a mesa, registra LOSS_CALOTE na auditoria com o valor perdido
- [x] Itens: quantidade, nome, badge KDS (Fila, Em Preparo, Pronto!, Entregue), adicionais, promoção, "Obs:", "R$ x cada • Bar/Cozinha", total, lixeira
- [x] Cancelar item com confirmação, estoque devolvido (inclui caixa fechada), auditoria
- [x] Rodapé: Total Acumulado, selo "10% incluso", Haver/Falta, botão "Fechar Conta / PDV"

## Lançar pedido (AddOrderModal.tsx, ModifierPickerModal.tsx)
- [x] Cabeçalho "LANÇAMENTO DE PEDIDO", Comanda #, mesa • cliente
- [x] Busca e categorias ("Todos os Itens")
- [x] Ícone da categoria (Beer, Wine, Flame, UtensilsCrossed, Beef, GlassWater)
- [x] Produto: praça (🍺 Bar / 🍳 Cozinha), quantidade já no carrinho, promoção (nome + preço riscado), "Com opções / adicionais", descrição
- [x] Produto com caixa: dois botões Unidade (R$) e "Cx Nx" (R$), com contagem de cada
- [x] Adicionais: obrigatório/opcional com regras, escolha única troca a opção
- [x] Carrinho "Itens a Enviar": N produto(s), Limpar, selo Cx/Un, opções, promoção, total da linha, − / +
- [x] Observação por item + chips rápidos (Com gelo e limão, Sem gelo, Bem gelada, Sem cebola)
- [x] "Total desta Rodada" e "Enviar para Bar / Cozinha"
- [x] Preço com promoção e adicionais igual ao do servidor
- [x] Leitor de código de barras / Enter com um resultado (extra do app nativo)

## Fechar conta (CheckoutModal.tsx)
- [x] Cabeçalho "FECHAMENTO & PDV", Comanda #, mesa • cliente, Imprimir, WhatsApp
- [x] Resumo: Subtotal (N itens), 10% com chave liga/desliga, Desconto com Aplicar, Total da Conta, Saldo Restante
- [x] Já Abatido (Haver) com lista de pagamentos (hora · forma · obs · valor)
- [x] Racha-Conta: Divisão Igual (1,2,3,4,5,6,8 → R$ por pessoa)
- [x] Racha-Conta: Por Itens (itens não pagos, − / + por item, subtotal selecionado + 10%), grava paidQuantity
- [x] Racha-Conta: Abater Valor (atalhos R$ 10/20/50/100 menores que o saldo, "Abatendo X → restará Y")
- [x] Formas: PIX Instantâneo, Cartão Crédito, Cartão Débito, Dinheiro, Voucher / Vale, Conta Fiado (Prazo)
- [x] Fiado: escolher cliente (com saldo atual) ou digitar nome de cliente novo (cria na hora)
- [x] Valor deste Pagamento + "Tudo"; Dinheiro: "Valor Entregue" e "Troco a Devolver"
- [x] Avisos: valor maior que o saldo; pagamento parcial (mesa continua ocupada)
- [x] Botão muda: "Abater R$ X da Conta" / "Registrar Pagamento Parcial" / "Confirmar Pagamento & Liberar Mesa"
- [x] Observação do pagamento como no Node: "Itens: …", "Haver (quitação)/(abatimento parcial)", "Divisão (Np)"
- [x] Compartilhar conferência no WhatsApp (abre o navegador com o texto)

## Transferir / Juntar (TransferModal.tsx, MergeModal.tsx)
- [x] Transferir comanda inteira para mesa livre
- [x] Juntar mesa ocupada (itens e haver vão juntos)
- [x] Textos e estados vazios do original ("Não há mesas livres…", "Não há outras mesas ocupadas…")

## Comandas finalizadas (OrderHistoryModal.tsx)
- [x] Lista de comandas pagas/fechadas com busca por comanda #, mesa, cliente ou garçom; Atualizar
- [~] Filtro por data pelo dia LOCAL (o Node usava UTC: vendas depois das 21h caíam no dia seguinte)
- [x] Detalhe: Data & Hora, Atendente, Cliente Identificado, itens (Qtd / Unit / Total), Subtotal, 10%, Desconto, Total Final, pagamentos
- [x] 2ª via do comprovante (ESC/POS na impressora do caixa)
- [~] Estorno: não existe no original (conferido no OrderHistoryModal.tsx)

## Editar locais e mesas (ManageTablesModal.tsx)
- [x] Lista por setor; editar número, nome, capacidade e setor da mesa
- [x] Excluir mesa (sem comanda aberta), com confirmação
- [x] Renomear setor; mover mesas para outro setor; criar setor novo ("+ Criar Novo Setor...")
- [x] Adicionar nova mesa

## Garçons e funcionários (ManageWaitersModal.tsx, ManageStaffModal.tsx)
- [x] Garçons: listar, criar (nome, código "Ex: 01", comissão), editar, remover/desativar
- [x] Funcionários: listar, criar (nome, perfil, senha/PIN, permissões por aba), editar, ativar/desativar, não desativar o único admin

## Impressão (ThermalReceipt.tsx, KitchenTicketReceipt.tsx, printService.ts)
- [x] Conferência de mesa / comprovante na impressora do caixa (ESC/POS)
- [x] Comanda da cozinha/bar ao enviar o pedido, com a "Auto-Impressão de Cozinha" ligada (Retaguarda; evento `kds:new_order`)
- [x] Recibo pedido pelo celular imprime no PC (servidor → print.rs)
