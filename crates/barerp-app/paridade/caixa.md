# Paridade: Caixa & PDV (`caixa`)

Original: `client/src/views/CashView.tsx` (1.007 linhas) e `client/src/components/QuickSaleModal.tsx` (1.050 linhas).
Port: `ui/pages/caixa.slint` (`CaixaStore`, `CaixaAreaPage`) + `src/pages/caixa.rs` + `barerp-db/src/caixa.rs`.
Dono: erp-rust-d5. Legenda: `[x]` feito, `[ ]` falta, `[~]` diferente de propósito (com o motivo).

## CashView: cabeçalho
- [x] Título "Frente de Caixa (PDV)" com selo "Turno Aberto" (verde) / "Caixa Fechado" (vermelho)
- [x] Subtítulo "Controle de suprimento, sangria, recebimentos e repasse de comissões (10%)"
- [x] Botão "Venda Rápida (Balcão)" com tecla F2 (só com o caixa aberto)
- [x] Botão "Comandas Finalizadas": abre o `OrderHistoryModal` da área Mesas (`MesasStore.open-order-history`)
- [x] Botão de atualizar, girando enquanto carrega

## CashView: caixa fechado (abertura)
- [x] Cartão "Abrir Turno de Caixa" com o texto de ajuda
- [x] Campo "Fundo de Troco Inicial (R$) *" com 200.00 de padrão, vírgula vira ponto, foco automático
- [x] Campo "Operador / Responsável *" (padrão "Operador de Caixa")
- [x] Campo "Observações do Turno (Opcional)" com o exemplo "Turno Noturno, Gaveta 01"
- [x] Botão "Confirmar Abertura de Caixa" com a tecla Ctrl+A; Enter no campo confirma
- [x] Validação "Informe um fundo de troco inicial válido"
- [x] Erro do servidor mostrado (ex.: caixa já aberto)
- [x] Link "Fechou o caixa anterior com erro de digitação? Reabrir último turno", com confirmação
      "Tem certeza que deseja reabrir o último turno de caixa fechado para corrigir valores?" e o aviso de sucesso

## CashView: caixa aberto (resumo)
- [x] Cartões: Fundo Inicial (Troco), Total de Vendas, Suprimentos (+) / Sangrias (−), Dinheiro Esperado na Gaveta
- [x] "Recebimentos por Forma de Pagamento": PIX, Cartão Crédito, Cartão Débito, Dinheiro em Espécie, Voucher / Outros
- [x] "Ações de Caixa" com "Operador: <quem abriu>"
- [x] Botão "⚡ Nova Venda de Balcão" (F2)
- [x] Botão "Efetuar Sangria (Retirada)" (Ctrl+S)
- [x] Botão "Efetuar Suprimento (Entrada)" (Ctrl+Shift+S)
- [x] Botão "Fechar Turno de Caixa" (Ctrl+F)
- [x] Cartão "Repasse de Comissões dos Garçons (10%)" com selo "Fim de Expediente" e "Total 10% do Turno"
- [x] Um cartão por garçom: iniciais, nome, "N comanda(s) atendida(s)", Total Vendido, Repasse (10%)
- [x] Vazio: "Nenhum valor de taxa de serviço (10%) acumulado neste turno ainda."
- [x] "Movimentações & Pagamentos do Turno": selo PAGAMENTO, mesa • forma, observação, + valor
- [~] Na lista de pagamentos, a forma aparece em português (PIX, Dinheiro, Crédito...) e a venda sem mesa aparece como
      "Balcão": o original mostrava o código (CASH) e "Mesa" também para o balcão
- [x] Vazio: "Nenhum pagamento registrado ainda neste turno."

## CashView: sangria / suprimento
- [x] Janela "Registrar Sangria (Retirada)" / "Registrar Suprimento (Entrada)"
- [x] Campo "Valor (R$) *" com foco automático; "Motivo / Justificativa *" com o exemplo de cada tipo
- [x] Validação "Informe o valor e o motivo da movimentação"
- [x] Cancelar / Confirmar; Esc fecha

## CashView: fechamento cego em 3 etapas
- [x] Etapa 1 "Fechamento de Turno": texto de ajuda, "Valor Total em Dinheiro Contado na Gaveta (R$) *" (grande),
      "Conferido Por *", Cancelar / "Avançar e Conferir"; validação "Informe um valor válido em dinheiro contado na gaveta"
- [x] Etapa 2 "Conferência do Caixa": esperado, digitado, diferença (verde = 0, azul = sobra, vermelho = falta)
- [x] Mensagem "Caixa 100% Exato!" ou "Falta/Sobra no Caixa Detectada" com o texto do original
- [x] "Voltar e Corrigir Valor" e "Confirmar Fechamento" (verde se exato, vermelho se diferente; "Fechando...")
- [x] Etapa 3 "Caixa Encerrado com Sucesso": esperado, contado, diferença registrada
- [x] "Repasse de 10% aos Garçons" com total e um item por garçom (comandas, vendas, repasse)
- [x] "Concluir e Fechar Janela" e "Fechou errado mesmo assim? Reabrir caixa para correção"

## CashView: atalhos
- [x] F2 venda rápida, Ctrl+S sangria, Ctrl+Shift+S suprimento, Ctrl+F fechar, Ctrl+A leva o foco ao fundo de troco
- [x] Atalho com o caixa fechado: "O caixa está fechado. Abra o turno antes de usar este atalho."
- [x] Ctrl+A com o caixa aberto: "O caixa já está aberto."
- [x] Atalho não abre uma janela por cima de outra
- [x] Atualiza sozinho quando outro aparelho mexe no caixa (`cash:updated`)

## QuickSaleModal: cabeçalho e avisos
- [x] Tela cheia "Venda Rápida de Balcão (PDV Tela Cheia)" com selo "PDV Expresso" e subtítulo
- [x] Fechar (X e Esc); com NFC-e sendo emitida, pede confirmação com o texto do original
- [x] Faixa verde "Venda #N finalizada com sucesso!" com "Troco: R$" e "Dispensar"
- [x] Faixa de NFC-e das últimas 4 vendas: "sendo emitida..." / "autorizada" / "em processamento: ..." / "não emitida: ..."
- [x] "Reimprimir" (autorizada), "Tentar de novo" (erro), dispensar (X)

## QuickSaleModal: catálogo
- [x] Busca com o texto de exemplo do original, foco automático, Ctrl+L, botão limpar
- [x] Busca por nome, código (com ou sem #), EAN, EAN da caixa, marca e descrição
- [x] Enter: EAN da caixa adiciona caixa; EAN/código exato adiciona unidade; senão o primeiro resultado
- [x] Enter não troca o produto enquanto a escolha de adicionais está aberta
- [x] Categorias: "Todos (N)" + uma por categoria
- [x] "Carregando cardápio..." e "Nenhum produto encontrado."
- [x] Cartão do produto: nome, #código, descrição, contador no carrinho, destaque quando está no carrinho
- [x] Produto com caixa: botões "Unidade" (preço, Nx) e "Cx 24x" (preço, N cx), estoque e preço por unidade da caixa
- [x] Produto simples: preço com promoção (preço cheio riscado), selo de estoque (vermelho se baixo)
- [x] Adicionais: abre a escolha antes de entrar no carrinho; pelo leitor só abre se houver pergunta obrigatória

## QuickSaleModal: carrinho e pagamento
- [x] "Itens da Venda (N)" e "Limpar"
- [x] Vazio: "Clique nos produtos ao lado para adicionar à venda rápida."
- [x] Linha: nome, selo Un/Cx, preço unitário, opções escolhidas, nome da promoção, − qtd +, total, remover
- [x] Mesmo produto + mesma unidade + mesmas opções soma na mesma linha
- [x] "Desconto (R$)" (limitado ao subtotal) e "Cliente (Opcional)"
- [x] Formas: PIX ⇧A, Dinheiro ⇧B, Crédito ⇧C, Débito ⇧D (Shift+letra não vale dentro de campo de texto, só na busca vazia)
- [~] Nos campos numéricos (desconto, valor entregue, CPF) Shift+letra digita a letra: o original tratava esses campos
      como "não texto" e trocava a forma de pagamento; no Slint o campo consome a tecla antes do atalho
- [x] Dinheiro: "Valor Entregue", notas R$20/R$50/R$100, "Exato", "Troco a Devolver"
- [x] "Total a Pagar" com o subtotal riscado quando há desconto
- [x] "Emitir NFC-e" (F8), desligada ao abrir; campo "CPF/CNPJ na nota (opcional)" com erro em vermelho
- [x] "Imprimir comprovante térmico" / "Imprimir comprovante se a NFC-e falhar" (marcado)
- [x] "Concluir Venda (+ NFC-e) (R$ X)" com F12; "Finalizando..."; desabilitado com o carrinho vazio
- [x] Conferências da NFC-e antes de gravar: CPF/CNPJ inválido, total zero, R$ 10.000 sem documento, produtos sem NCM
- [x] Preço com promoção recalculado na hora; 409 do servidor quando a promoção muda mostra a mensagem
- [x] Depois da venda: NFC-e em segundo plano (DANFE impresso se autorizada) ou comprovante; limpa carrinho,
      desconto, valor entregue, cliente e CPF; avisa o caixa para atualizar
- [x] Pedido aparece na cozinha (kds:new_order) e o caixa dos outros aparelhos atualiza (cash:updated)

## Diferenças gerais
- [~] Valores em reais no formato brasileiro (`R$ 1.234,50`); o original mostrava `R$ 1234.50` (`toFixed(2)`)
- [~] Ícones como símbolos (⚡ 🔒 💳 ▦) no lugar dos ícones do lucide-react
- [~] `alert()` e `window.confirm()` viram janelas do próprio app ("Aviso" / "Confirmar"), fechadas com Esc
- [~] Esc também fecha a venda rápida (no original só o X); com NFC-e emitindo, pede a mesma confirmação
- [~] "Conferido Por" e "Operador / Responsável" vêm preenchidos com o usuário logado
