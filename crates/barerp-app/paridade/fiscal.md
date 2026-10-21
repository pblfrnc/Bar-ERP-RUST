# Paridade — Módulo Fiscal (`fiscal`)

Dono: erp-rust-aa. Tela nativa: `ui/pages/fiscal*.slint` + `src/pages/fiscal.rs` (+ `src/pages/fiscal/`).
As telas usam as mesmas rotas `/api/fiscal` do servidor (`barerp-server/src/fiscal/`), chamadas dentro do
processo por `ServerHandle::fiscal`, então a regra fiscal é uma só para o app e para os celulares. Ícones:
Lucide 0.469 (os mesmos do original), em `ui/pages/fiscal_icons.slint`.

Legenda: `[x]` feito · `[ ]` falta · `[~]` diferente de propósito (com o motivo).

## FiscalHubView (Módulo Fiscal Central)
- [x] Título "Módulo Fiscal Central" + subtítulo; selo SEFAZ no topo
- [x] RECEBIMENTO FISCAL: 1º BIP, 2º BIP (Importação sem chave) e Notas Pendentes (legenda XML Baixado/Pendente)
- [x] EMISSÃO: Emitir NF-e (Modelo 55) e Emitir NFC-e (Modelo 65)
- [x] REIMPRESSÃO: Reimprimir NFC-e (Cupom) e Reimprimir NF-e (Nota)
- [x] CANCELAMENTO: Cancelar NFC-e (30 min) e Cancelar NF-e (24h)
- [x] INUTILIZAÇÃO: Inutilizar NFC-e e Inutilizar NF-e (modais)
- [~] "Tabela NCM & Alíquotas": abre o NcmLookupModal da área Cardápio (por cima da tela de produtos)
- [x] Painel do Contador e Configurações Fiscais
- [x] "Importar Itens" do Recebimento/Pendentes abre a Importação já com a chave
- [~] Emoji decorativos dos títulos ("⚡", "📋") saíram: a fonte do app não tem emoji

## SefazStatusBadge
- [x] `/sefaz-status` ao abrir e a cada 5 minutos; rótulos "SEFAZ...", "SEFAZ Online (UF)", "SEFAZ (UF) Fora do Ar", "Servidor ERP Desconectado"
- [x] Cores cinza/verde/vermelho; clique abre detalhes (autorizador, recarregar, motivo, detalhe do token, código, latência, dica)

## NfceInutilizacaoModal / NfeInutilizacaoModal
- [x] Textos de cada modelo; série da configuração; Nº inicial copia para o final; contagem "Serão inutilizadas N notas" (NF-e)
- [x] Contador "N/15 min"; validações (série/intervalo, final ≥ inicial, 15 caracteres); faixa verde/vermelha com o resultado
- [~] O original lia `data.serie` para a NFC-e (campo inexistente, ficava "1"); aqui usa `serieNfce`

## ManualNfceView (Emissor Fiscal de NFC-e em tela cheia)
- [x] Cabeçalho (voltar, título, "SEFAZ ONLINE", subtítulo) e "Limpar Itens"
- [x] Busca por nome/código/EAN/marca com foco automático, contador, ×; Enter: código exato → 1 resultado → lista; Esc fecha
- [x] Lista (até 10): nome, #código, marca, NCM, CFOP, preço, "+ Adicionar"; "Nenhum produto encontrado"
- [x] Itens: nome, #código, NCM ou "NCM pendente", CFOP, −/+, unitário, subtotal, remover; títulos com unidades e produtos; estado vazio
- [x] CPF/CNPJ com máscara e validação; obrigatório a partir de R$ 10.000,00; mensagens de ajuda; nome opcional
- [x] Pagamento PIX / Dinheiro / Cartão Crédito / Cartão Débito; total; "Emitir Cupom Fiscal Agora" / "Transmitindo à SEFAZ..."
- [x] Validações antes de emitir; modal "Produto sem NCM" que salva no cadastro e emite
- [x] Em processamento: aviso "NÃO emita novamente" com a referência e nova venda
- [x] Autorizada: "Venda Finalizada com Sucesso!" com total, forma, chave + Copiar Chave; imprimir/reimprimir; abrir DANFE; e-mail; "Nova Venda (Zerar)" com Enter/Esc
- [~] Impressão: o original mandava o HTML da Focus ao Electron; aqui o DANFE sai em ESC/POS a partir do XML autorizado (`imprimir_danfe_nfce`), com QR Code
- [~] Avisos que eram `alert()` aparecem numa faixa na própria tela

## EmitNfeView (Emissão de NF-e Modelo 55)
- [x] Cabeçalho "SEFAZ ONLINE • FOLHA A4", busca e itens (os mesmos componentes do emissor de NFC-e)
- [x] CPF/CNPJ do comprador com validação ao digitar ("Digitando CPF (n de 11)..."), CPF procura o cliente cadastrado, CNPJ consulta a Receita e preenche nome e endereço; botão "Buscar CNPJ"
- [x] Nome/Razão Social obrigatório; endereço (CEP com auto-completar BrasilAPI → ViaCEP, logradouro, número, bairro, complemento, município, UF)
- [x] Pagamento; total; "Emitir NF‑e"; validações do original (documento, dígitos, nome)
- [x] Pós-emissão: "NF‑e emitida! Referência", chave (consulta a cada 2,5 s enquanto processa), Copiar Chave, Imprimir DANFE A4, e-mail, "Nova Emissão (Zerar)" com Enter/Esc
- [~] NCM: o original mandava `22030000` (cerveja) para qualquer produto sem NCM; aqui abre o mesmo cadastro de NCM do emissor (dado fiscal não é inventado)
- [~] "Selecionar Impressora": o DANFE A4 abre no visualizador de PDF do sistema, que já escolhe a impressora

## FiscalImportView (2º BIP — Importação de NF-e e Entrada no Estoque)
- [x] Tela inicial: bipe da DANFE (44 dígitos, carrega sozinho ao completar), "Carregar Itens", contador "N/44 dígitos", "Leitor USB pronto"
- [x] Contingência: escolher arquivo .xml, Trocar, Carregar XML
- [x] "Buscando e Processando XML da SEFAZ..." (parsed-by-chave, com reserva XML bruto + import-xml)
- [x] Conferência: fornecedor, CNPJ e NF; 2º bip de confirmação (confere / divergente / aguardando) que libera a entrada
- [x] Por item: Vincular Existente / Criar Novo / Ignorar; produto (lista com estoque atual) ou categoria
- [~] Produto para vincular escolhido por busca (nome, código, código de barras ou marca, sem acento; Enter pega o
  primeiro; sem texto, os parecidos com o item da nota vêm primeiro) em vez da lista suspensa (pedido do Pablo, 21/10/2026)
- [x] NCM, CFOP, código de barras e unidade da nota vão para o produto criado ou vinculado
- [~] O NCM da nota aparece em cada item; ao vincular a um produto que já tem outro NCM válido, o usuário escolhe se fica o
  da nota ou o do cadastro (o original trocava sem avisar). Item criado sem NCM na nota mostra o aviso para cadastrar
- [x] Fator de conversão: liga/desliga, atalhos (10/12/6/24 un, 12/6 porções), fator digitado, custo unitário real, entrada no estoque; sugestão automática pelo nome (CX12, FD 24, cigarro, kg)
- [x] Preço: margem ↔ preço (um recalcula o outro), venda atual e custo anterior, "Preço abaixará / Reajuste / Preço mantido"; caixa proporcional
- [x] Validações e modal "Atenção: Redução de Preço de Venda!" antes de efetivar
- [x] Entrada efetivada numa transação só (estoque, custo, preço, caixa, histórico de preço com a chave, nota finalizada)

## NfReceivingView (1º BIP)
- [x] Fluxo em 2 etapas; bipe com Enter; "Registrar NF"; upload manual leva à Importação
- [x] Resultado: erro / ciência registrada (XML em processamento, "Tentar Conferir Agora") / concluído ("Ir para 2º Bip")
- [x] Histórico: status, NF, emitente, chave, valor ("Aguardando SEFAZ..."), data, "Buscar XML", "Conferir"
- [~] Status "importada" do original virou "finalizada" (o banco Rust só usa finalizada); o botão fica desabilitado nela

## PendingInvoicesView (Notas Pendentes DF-e)
- [x] Sincronizar com SEFAZ (aviso some em 7 s, botão Fechar); métricas clicáveis; abas de status com contagem; filtro de XML
- [x] Busca (fornecedor, CNPJ, número, chave), datas, Filtrar, Limpar, períodos rápidos (Hoje, 7 dias, 30 dias, Este Mês)
- [x] Legenda das bolinhas e "Exibindo N de M"; carregando; vazio com "Sincronizar Agora"
- [x] Linha: XML Baixado/Pendente, status, NF/série, emitente, CNPJ, chave + copiar, emissão, valor
- [x] Ações: Receber (1º Bip), 2º Bip, Já dei entrada, Reabrir, Buscar XML, baixar XML, ver DANFE (com confirmações)
- [~] Datas digitadas como AAAA-MM-DD (o navegador tinha um seletor de data)

## NfceReprintView / NfeReprintView
- [x] Voltar, "Atualizar Notas"; busca por número (NF-e: número/referência/chave + série opcional)
- [x] Nota encontrada: série, status, valor, chave; NFC-e "Imprimir Direto (Silencioso)" (ESC/POS) e "Ver Cupom"; NF-e "Imprimir DANFE A4" e "Ver PDF"
- [x] Recentes com status colorido, data, valor e "Reimprimir"

## NfceCancelView / NfeCancelView
- [x] Avisos de prazo (30 min / 24 h); lista com prazo restante (cores de urgência na NF-e); "Cancelar Esta Nota"
- [x] NF-e: localizar por número/chave/referência fora da lista; recusa nota já cancelada
- [x] Dupla confirmação com dados da nota, justificativa com contador e "A SEFAZ exige ao menos 15 letras"; resultado no topo

## AccountantPanelView (Painel do Contador)
- [x] Mês de competência (‹ ›, AAAA-MM), resumo NFC-e / NF-e / Entradas
- [x] Posição de estoque e lucro previsto (por categoria), opção "Enviar posição do estoque no SPED"
- [x] Avisos de mês sem notas / notas prontas; "Baixar Fechamento Contábil Local (.ZIP)" (pergunta onde salvar)
- [x] Backups oficiais da Focus: atualizar, lista por competência, baixar XMLs / DANFEs
- [~] O .zip não inventa XML de nota que falta: lista em XMLS_FALTANDO.txt

## FiscalSettingsView (Configurações Fiscais)
- [x] Testar Conexão (resultado, empresas, status do CNPJ, passos quando não localizado, Cadastrar Empresa Agora, Diagnóstico Focus NFe, dicas de token)
- [x] Token; Razão Social; Nome Fantasia (destaque no cupom); CNPJ com máscara, validação e preenchimento pela Receita; IE; CRT; CSC e ID; ambiente
- [x] Série e próximo número de NF-e e NFC-e com as dicas; endereço
- [x] Certificado A1: escolher .pfx/.p12, senha, enviar à Focus, resultado
- [x] "Cadastrar Dados da Empresa" (cadastra na Focus e salva; bloqueado com CNPJ inválido)

## Fora da área fiscal (registrado para não se perder)
- [~] QrScannerModal: só é usado pelo ServerConfigModal (área Retaguarda), para ler o QR de configuração do servidor
- [x] ImportXmlModal ("Importar do Sistema Antigo", uso exclusivo do desenvolvedor): botões "Importar XML" de Produtos e de Fornecedores (só ADMIN), janela `ui/pages/import_legacy.slint`, regras em `barerp-db/src/legado.rs` e rota `/api/import-xml/tabelas`
- [~] Importar do Sistema Antigo: escolhe o arquivo pelo diálogo do sistema (sem arrastar e soltar) e grava tudo numa transação só (um erro não deixa metade importada)

## Para outras áreas (pronto)
- [x] `pages::fiscal::emitir_nfce_da_venda` (venda de balcão do Caixa, igual ao `emitNfceForOrder`)
- [x] `pages::fiscal::imprimir_danfe_nfce` (DANFE NFC-e em ESC/POS, com QR Code, a partir do XML autorizado)
- [x] Consulta de NCM para Produtos: `ServerHandle::fiscal("GET", "/ncm/{codigo}", Corpo::Nada)`
