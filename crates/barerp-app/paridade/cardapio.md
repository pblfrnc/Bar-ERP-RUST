# Paridade: Cardápio & Estoque (`cardapio`)

Origem: `client/src/views/ProductsView.tsx` (+ `ModifierGroupsManager`, `PromotionsManager`, `StockSheetModal`,
`DigitalMenuItemsModal`, `ProductPhotoField`, `NcmLookupModal`, `views/SuppliersView.tsx`, `views/CustomersView.tsx`)
e as rotas do servidor que elas chamam (`server/src/routes/products.ts`, `modifiers`, `promotions`, `suppliers`,
`customers`, `fiscal` (NCM/CNPJ), `services/catalogService.ts` (EAN)).

Legenda: `- [x]` feito · `- [x]` falta · `- [~]` diferente de propósito (com o motivo).
Dono: erp-rust-63. Tela: `ui/pages/cardapio*.slint` · lógica: `src/pages/cardapio.rs` · banco: `barerp-db/src/products.rs` (+ `modifiers.rs`, `promotions.rs`).

## 1. Lista principal

### Cabeçalho
- [x] Ícone + título "Cardápio & Estoque" + subtítulo "Controle de preços, códigos internos, EAN de barras e tributação fiscal"
- [x] Botão em destaque "Novo Produto" (abre o cadastro)
- [x] Barra de ações secundárias (mesmo tamanho, com ícone colorido):
  - [x] Relatório de Estoque (modal de estoque e lucro previsto)
  - [~] Importar XML: o botão leva para a tela Fiscal, onde fica o importador (`ImportXmlModal` é da área fiscal)
  - [x] Organizar Bar/Cozinha (com "carregando"; mensagem de resultado)
  - [~] Fornecedores (com contador): abre a aba Fornecedores da área em vez de um modal (a aba junta o modal do ProductsView e a SuppliersView)
  - [x] Adicionais e Perguntas (gerenciador de grupos)
  - [x] Promoções do Dia (gerenciador de promoções)
  - [x] Cardápio Digital (itens do cardápio por QR)
  - [x] Folha de Balanço (PDF)
  - [x] Nova Categoria
  - [x] Gerar Códigos (confirmação; atribui códigos sequenciais só a quem não tem; "Gerando...")
  - [x] Corrigir Duplicados (fica vermelho com contador quando há códigos repetidos; confirmação; "Corrigindo...")

### Filtros
- [x] Busca por nome, código interno, EAN, marca, fornecedor ou descrição (sem diferenciar maiúsculas)
- [x] Chip "Todas (N)"
- [x] Chip por categoria com faixa `#1001+`, botão editar (lápis) e excluir (lixeira) no próprio chip

### Alerta de duplicados
- [x] Faixa vermelha "Atenção: Detectamos produtos com numeração interna duplicada (N produtos)" + "⚡ Corrigir Numerações Agora"

### Tabela de produtos
- [x] Colunas: Produto · Categoria · Destino KDS · Preço Venda · Custo · Estoque · Ajuste Rápido · Ações
- [x] Produto: nome + `#código`; marca (âmbar); fornecedor (selo verde com caminhão); EAN (selo com código de barras); descrição curta
- [x] Clique no nome abre a edição
- [x] Categoria (selo cinza; "Geral" sem categoria)
- [x] Destino KDS: selo Bar (âmbar) / Cozinha (verde)
- [x] Preço venda; abaixo "Cx 12x: R$ 84,00" quando tem caixa fechada
- [x] Custo + selo "NN% margem" (verde ≥50, âmbar ≥30, vermelho abaixo)
- [x] Estoque com unidade; vermelho com alerta quando ≤ mínimo; "Composto (Ficha)" para produto com receita
- [x] Ajuste rápido −1 / +10 (não aparece em produto composto)
- [x] Ações: Histórico de preços · Duplicar · Editar · Excluir (com confirmação)
- [x] Lista vazia: "Nenhum produto cadastrado encontrado para ..." + "+ Cadastrar Novo Produto"
- [x] Lista vazia com código de barras digitado (7 a 14 dígitos): "Cadastrar Código N" abre o cadastro já com o EAN e faz a busca automática

## 2. Cadastro / edição de produto (tela inteira)

### Topo
- [x] Voltar · título "Editar: <nome>" com selo `#código`/"SEM CÓDIGO", ou "Cadastrar Novo Produto" com selo "NOVO ITEM"; subtítulo
- [x] Excluir (só na edição) · Cancelar · Salvar Produto

### Preenchimento automático por código de barras
- [x] Campo EAN (Enter = Busca Geral)
- [x] "Puxar das Notas (XML)" · "Puxar Catálogo Global" · "Busca Geral" (desabilitados sem EAN; "carregando" no botão usado)
- [x] Preenche nome, código (se vazio), EAN, marca, fornecedor (e vincula o cadastrado parecido), NCM, CEST, CFOP, unidade, descrição (se vazia), destino KDS, categoria sugerida, custo, preço sugerido (se vazio)
- [x] Não encontrado: preenche só código/NCM/CFOP vazios e mostra a mensagem
- [x] Caixa de retorno verde/vermelha/âmbar com a mensagem

### Identificação & Básico
- [x] Nome* (com volume em ml/l no nome manda para o Bar e troca categoria Cozinha → Bar)
- [x] Código interno (maiúsculas) + dica
- [x] Categoria* com faixa "(Faixa #1001)" + "Nova"; trocar a categoria num produto novo gera o próximo código da faixa
- [x] Marca / Fabricante
- [x] Unidade (minúsculas) + sugestões un, lata, garrafa, dose, copo, torre, porção, kg, l

### Preços, custos & rentabilidade
- [x] Custo (R$): recalcula a venda pela margem alvo
- [x] Margem alvo (%) "auto-cálculo": recalcula a venda
- [x] Venda (R$)*: recalcula a margem alvo
- [x] Simulador: Lucro bruto · Margem de lucro (cores) · Markup s/ custo
- [x] "Usar Custo da Receita (R$ X)" quando há ficha técnica com custo

### Venda por caixa / fardo fechado
- [x] Liga/desliga
- [x] Qtd por caixa* + atalhos 6, 12, 15, 24, 30
- [x] Preço da caixa* + "Sai a R$ X por unidade (Economia de R$ Y/un)"
- [x] EAN da caixa (DUN-14) + dica de que o caixa baixa N unidades

### Fator de conversão & fracionamento
- [x] Texto explicativo; atalhos 10 un (Cigarro), 12 un (Caixa), 6 un (Pack), 24 un (Fardo), 12 porções (5kg), 6 porções (2kg)
- [x] Fator sugerido ao abrir a edição (caixa cadastrada, cigarro 10, cx 12/24/6, kg → porções)
- [x] Rótulo muda entre "Unidades por Caixa / Pacote" e "Qtd. de Porções no Peso/Embalagem"
- [x] "Aplicar Conversão (Nx)": estoque × N, custo ÷ N, venda pela margem alvo, liga caixa com N (preço da caixa = custo × 1,5 se vazio)
- [x] "Desfazer" + aviso verde do que foi aplicado

### Ficha técnica & composição
- [x] Liga/desliga + texto explicativo
- [x] Linha por insumo: escolha do produto (sem o próprio e sem compostos) com estoque e custo; −/+ na quantidade; unidade; subtotal; remover
- [x] Escolher um insumo já listado soma as quantidades e junta as linhas
- [x] "Custo Somado" e "Adicionar Insumo / Ingrediente"
- [x] Ao salvar, insumos repetidos são somados

### Adicionais e perguntas
- [x] "Gerenciar grupos"; lista de grupos com marcação na ordem (1., 2., ...), regra do grupo e nº de opções; inativo esmaecido
- [x] Só regrava os grupos do produto quando a seleção mudou

### Cardápio digital (no produto)
- [x] "Mostrar no cardápio"
- [x] Foto: escolher (JPEG/PNG/WebP, reduzida antes de gravar), trocar, remover; enviada depois de salvar
- [x] Descrição (até 500) + dica

### Fornecedor & compras
- [x] Escolher fornecedor cadastrado ("Fantasia (Razão) • documento") ou "Nenhum"
- [x] Cartão do fornecedor escolhido: nome, documento, contato, "Falar no WhatsApp"
- [x] Sem fornecedor escolhido: nome avulso
- [x] "+ Novo": fornecedor rápido (razão*, fantasia, documento, telefone) que já fica vinculado
- [x] Ao salvar: nome avulso igual a um cadastrado vincula; cadastrado sem nome preenche o nome

### Estoque & destino KDS
- [x] Destino: Bar / Cozinha / Direto (sem KDS)
- [x] Estoque atual · Estoque mínimo + dica

### Tributação fiscal
- [x] NCM (8 dígitos) com "Buscar NCM" e conferência automática ao completar 8 dígitos ("Verificando...", descrição + IBPT, "NCM não localizado")
- [x] CEST · CFOP (4 dígitos) + atalhos 5102, 5405, 5101

### Rodapé fixo
- [x] Nome + código, venda, custo, margem colorida · Excluir · Cancelar · "Salvar Alterações"/"Cadastrar Produto"

### Gravação (servidor)
- [x] Criar/alterar com todos os campos; validação de nome, preço e categoria
- [x] Histórico de preço gravado quando venda ou custo mudam (motivo edição manual, responsável)
- [x] Ficha técnica regravada
- [x] Duplicar: copia tudo (menos EAN e foto), nome "(Cópia)", código novo da faixa, aviso
- [~] Excluir produto: o que já foi vendido é só desativado (o Node apagava os itens das comandas antigas, inclusive pagas, e corrompia relatórios, caixa e fiscal); insumo de ficha técnica de outro produto não é apagado

## 3. Categorias
- [x] Modal Nova/Editar Categoria: nome*, faixa inicial* com `#N+`, atalhos 1001…7001, texto do que acontece
- [x] Nova: sugere a próxima faixa livre (1001, 2001, ...)
- [x] Editar: "Renumerar produtos existentes desta categoria"
- [~] Excluir categoria: apaga os produtos nunca vendidos; com produto já vendido é recusado com a mensagem para mover os produtos (mesmo motivo acima)

## 4. Códigos internos
- [x] Próximo código da faixa da categoria
- [x] Gerar códigos para quem não tem
- [x] Corrigir duplicados / sem código
- [x] Renumerar uma categoria

## 5. Estoque
- [~] Ajuste rápido (−1 / +10): funciona (a rota `PUT /products/:id/stock` que a tela do Node chamava não existia no servidor)
- [x] Organizar Bar/Cozinha (regra do servidor)
- [~] Relatório de estoque & lucro previsto: 4 cartões + tabela por categoria; mostra os valores de verdade (a tela do Node lia campos que a rota não mandava: cartões em R$ 0,00 e categorias sem nome)

## 6. Histórico de preços (modal)
- [x] Título com produto e código; carregando; vazio com explicação
- [x] Cada alteração: data, "Por:", selo "Entrada NF-e (XML)"/"Edição Manual"; venda antiga riscada → nova com diferença ↑/↓ e %; custo antigo → novo com "Custo subiu/caiu"; chave da NF-e

## 7. Adicionais e perguntas (`ModifierGroupsManager`)
- [x] Lista: nome, selo da regra (Obrigatório/Opcional · escolha 1/sem limite/até N), inativo, opções com preço, "Usado em N produto(s)"
- [x] Novo grupo + modelos "Ponto da carne", "Gelo e limão", "Adicionais"
- [x] Edição: nome, máximo de escolhas (1 = única, 0 = sem limite), obrigatória, ativo; opções com subir/descer, nome, +R$, ativa, remover; "Adicionar opção"
- [x] Validações: nome, ao menos uma opção, obrigatória precisa de opção ativa
- [x] Excluir grupo (avisa em quantos produtos está)

## 8. Promoções do dia (`PromotionsManager`)
- [x] Lista: nome, selo "Agora" quando está valendo, descrição (dias, horário, desconto), alvos (até 4 + "e mais N"), liga/desliga, editar, excluir
- [x] Edição: nome, tipo (% ou preço fixo), valor, ativa, dias (D S T Q Q S S) + "Todos"/"Fim de semana", início/fim (vira a noite), categorias inteiras, produtos (busca por nome/código, até 30 resultados, chips removíveis)
- [x] Prévia com até 3 produtos (de → para; aviso quando não fica mais barato) e total afetado
- [x] Validações iguais (nome, valor > 0, % < 100, dias, horário, alvos)

## 9. Folha de balanço (`StockSheetModal`)
- [x] Categoria (todas ou uma), ordem (alfabética / por categoria), mostrar custo
- [x] Texto do que sai na folha; "Gerar PDF (N produtos)" / "Nenhum produto"; mensagem de resultado
- [x] PDF pelo `barerp-report` e salvar/abrir no computador

## 10. Itens do cardápio digital (`DigitalMenuItemsModal`)
- [x] Busca (sem acento); filtros Todos / No cardápio / Fora do cardápio / Sem foto; "N de M no cardápio"
- [x] Comidas antes de Bebidas; seção por categoria com "Marcar/Desmarcar todos" e aviso de categoria escondida
- [x] Subgrupos automáticos pelo nome (`menuGroups.ts`) com "(x de y no cardápio)" e marcar/desmarcar
- [x] Linha: marcar, miniatura, nome, preço · descrição, "Sem preço de venda: não aparece"
- [x] "Foto e descrição": foto (escolher/remover na hora) e descrição com "Salvar descrição"
- [x] Aviso de itens que parecem estoque da loja (cigarro, limpeza, ...) com "Desmarcar esses N"
- [x] "Salvar N alteração(ões)" / "Nada para salvar"; perguntar ao fechar com alterações

## 11. Consulta de NCM (`NcmLookupModal`)
- [x] Campo (só dígitos, até 8), Consultar (mín. 4 dígitos)
- [x] Atalhos comuns no bar (11 NCMs)
- [x] Resultado: código, "Válido na SEFAZ", descrição, imposto federal/estadual; "Usar este NCM no Produto"
- [x] Erro em vermelho

## 12. Fornecedores (modal da lista + `SuppliersView`)
- [x] Cabeçalho "Gestão de Fornecedores" + "Novo Fornecedor"
- [x] Cartões: total, com CNPJ/CPF, produtos vinculados (do `SuppliersView`)
- [x] Busca por razão, fantasia, CNPJ, contato, cidade, telefone
- [x] Cartão: nome (fantasia/razão), CNPJ formatado, "N produtos vinculados", telefone (WhatsApp), e-mail, contato, cidade-UF, observações; Editar; Excluir (confirmação)
- [x] Vazio: mensagens + "+ Cadastrar Primeiro Fornecedor"
- [x] Formulário: razão*, fantasia, CNPJ/CPF com "Consultar CNPJ" (preenche os vazios), telefone, IE, e-mail, contato, cidade, UF, endereço, observações
- [~] Importar XML de fornecedores (só desenvolvedor): fica no importador da área Fiscal

## 13. Clientes / fiado (`CustomersView`)
- [x] Cabeçalho "Clientes (Fiado)" + "Novo Cliente"
- [x] Busca por nome
- [x] Linha: nome, telefone, documento, "Saldo Devedor" (vermelho se deve)
- [x] Cadastrar cliente (nome*, telefone, documento)
- [~] Receber pagamento do fiado: não existia tela no Node (o saldo só subia); a tela Rust ganha "Receber" por cliente (`customers::pay_credit_tab`)
