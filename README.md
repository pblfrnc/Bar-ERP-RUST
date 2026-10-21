# BarERP Pro nativo (Rust + Slint)

Reescrita do BarERP (Node + Electron + React) em Rust, com telas nativas em Slint (sem webview nem Chromium).
Decidida em 18/10/2026. Desde 21/10/2026 esta pasta é o repositório **privado** `pblfrnc/Bar-ERP-RUST` (sem CI nem
releases). Push no `pblfrnc/bar-erp` continua proibido: cada push na `main` de lá gera uma release que o PC do cliente
baixa sozinho.

## Decisões
- **Interface:** Slint, desenhada pelo próprio app. Licença royalty-free: exige o crédito "Made with Slint" na tela Sobre.
- **Celulares:** os APKs de Garçom e Cozinha (React + Capacitor) continuam como estão. O servidor Rust precisa
  manter **a mesma API REST e o mesmo Socket.IO** do `server/` atual.
- **Banco:** o mesmo `bar.db`. Ele abre sem conversão: datas em ms (Prisma) ou texto (`CURRENT_TIMESTAMP`),
  booleanos 0/1, ids cuid e campos cifrados `$enc$v1$`/`$pbkdf2$` compatíveis.
- **Migração:** tudo junto, com troca única no final, quando houver paridade de funções.

## Crates
| Crate | O que faz | Quem |
|---|---|---|
| `barerp-core` | Regras puras: preço/promoção/adicionais, CPF/CNPJ/CEST, cofre AES-GCM + PBKDF2, cuid, datas, R$ | principal |
| `barerp-db` | sqlx/SQLite, migrações por `sql/schema.sql`, login/senha, mesas, comandas (abrir, lançar, cancelar, 10%, desconto, transferir, juntar), catálogo, auditoria, eventos | principal (`notas.rs`/`importacao.rs`: sessão fiscal) |
| `barerp-app` | Executável `barerp` (Slint): login, as seis áreas do menu (`src/pages`, `ui/pages`), servidor e serviços de fundo embutidos | principal |
| `barerp-print` | ESC/POS: comprovante, comanda da cozinha, QR nativo, gaveta, logo; TCP 9100, spooler RAW do Windows e CUPS (macOS/Linux), com a impressora padrão do sistema | sessão de apoio |
| `barerp-report` | PDF A4 sem webview: folha de balanço e QR Codes das mesas | sessão de apoio |
| `barerp-backup` | VACUUM INTO, pasta externa, retenção, agendador e **restauração** | sessão de apoio |
| `barerp-menu-publish` | Publicação do cardápio digital no Netlify | sessão de apoio |
| `barerp-license` | Licença: ID da máquina igual ao `node-machine-id`, chave offline/legada, painel do Render | principal |
| `barerp-updater` | Atualizador: release do GitHub (3 estratégias do `updater.cjs`), download com progresso, instalador | principal |
| `barerp-fiscal` | Cliente Focus NFe, payloads NFC-e/NF-e, NF-e recebidas, leitura de XML | sessão fiscal |
| `barerp-server` | API REST + Socket.IO iguais às do Node, para os APKs | sessão de servidor |

Diferenças de propósito em relação ao Node estão documentadas no topo de cada módulo
(ex.: `barerp-db/src/orders.rs`: transações, devolução de estoque de caixa fechada).

## Comandos
```bash
source "$HOME/.cargo/env"
cargo test --workspace
cargo run -p barerp-app                      # abre o app (banco em %APPDATA% ou BARERP_DB=<arquivo>)
cargo run -p barerp-db --example inspect -- <cópia do bar.db>
# prints das telas sem abrir janela (o macOS bloqueia captura de tela), temas escuro e claro:
cargo run -p barerp-app --example mesas_shots -- <cópia do bar.db> <pasta>
cargo run -p barerp-app --example cardapio_shots -- <cópia do bar.db> <pasta>
cargo run -p barerp-app --example kds_shots -- <pasta>          # dados inventados
cargo run -p barerp-app --example retaguarda_shots -- <pasta>   # dados inventados
```
Os exemplos aplicam as migrações no arquivo, então use sempre uma **cópia** do banco do cliente.

## Roteiro até a troca
1. **Operação diária** (prioridade): ~~detalhe da mesa, lançar itens (adicionais e promoção), transferir/juntar~~ (feito),
   conferência impressa, checkout (dividir, haver, taxa de 10%), venda de balcão com atalhos, caixa (abertura, sangria,
   suprimento, fechamento cego), KDS.
2. **Servidor embutido** (`barerp-server`, axum + socketioxide): a mesma API e os mesmos eventos (`table:updated`,
   `order:updated`, `cash:updated`, `kds:*`, `print:receipt`) para os APKs, mais o QR de conexão dos celulares.
3. **Fiscal**: emissão de NFC-e na venda e no emissor manual, NF-e, cancelamento, inutilização, reimpressão
   (DANFE NFC-e em ESC/POS; NF-e A4 pelo PDF da Focus), 1º/2º bip, importação de XML, painel do contador.
   Homologação na Focus antes da troca.
4. **Retaguarda**: produtos/estoque/ficha técnica, fornecedores, clientes/fiado, funcionários e permissões,
   dashboard, auditoria, configurações (impressoras, fiscal, backup, cardápio digital), licença.
5. **Serviços de fundo**: backup (`VACUUM INTO` + pasta externa), agendador de NF-e recebidas, publicação do
   cardápio digital, checagem de licença, atualizador.
6. **Instalador Windows**: ~~NSIS com ícone, instalando por cima e mantendo `%APPDATA%`~~ (feito, `installer/barerp.nsi`).
   Validar a pasta real do `bar.db` no PC do cliente (`BarERP Pro`, `bar-erp` ou `BarERP`).

## Decisões do Pablo (18/10/2026)
- **Sem senha mestra `9999`** (no Node ela entrava em qualquer usuário). O Administrador inicial nasce com **1234**,
  o sistema sugere trocar no primeiro acesso e cada um troca a própria senha em "Trocar senha".

## Pendências para decidir
- **Correções fiscais em relação ao Node** (`barerp-fiscal`, a confirmar com o Pablo): NCM obrigatório também
  na NF-e, mesmo mapa de pagamentos da NFC-e, `processando_*` tratado como processando, erro de cancelamento com
  HTTP 200 vira erro, NCM/EAN do XML preservados como texto.
- **PIX dinâmico:** o README antigo cita um "PIX dinâmico", mas não existe gerador de QR PIX no código atual.
  Fazer isso seria uma funcionalidade nova.

---

# PASSAGEM PARA OUTRO AGENTE (estado em 20/10/2026)

## Regras obrigatórias
1. **Nada de push no `bar-erp`.** NUNCA `git push`, deploy Netlify/Render ou release no `pblfrnc/bar-erp`: push na
   `main` de lá gera release que o PC do cliente (JUBAR) baixa sozinho. A reescrita vive no repo privado
   `pblfrnc/Bar-ERP-RUST`; commit e push lá só quando o Pablo pedir, e nada de CI ou release sem ele decidir.
2. **Nenhuma chamada real à Focus NFe** (nem homologação) sem o Pablo autorizar. Testes com wiremock.
3. **Nunca abrir o banco real do cliente**; use cópias (`BARERP_DB=<cópia>`). As migrações gravam no arquivo.
4. **Não mexer no projeto Node** (`server/`, `client/`, `electron/`): ele segue em produção até a troca.
5. Rodar o cargo com `source "$HOME/.cargo/env"`. Versões de dependência estáveis e conhecidas (ver Cargo.toml
   dos crates); sqlx 0.8, Slint 1.x, axum 0.7, reqwest 0.12 + rustls.
6. Nunca descartar nem inventar dado fiscal (NCM, CPF/CNPJ, valor, XML). Só `autorizado` é sucesso.

## Armadilhas já conhecidas
- **Datas no bar.db:** o Prisma grava inteiro em ms e o SQL antigo grava texto. Use sempre `barerp_db::DbTime`.
- **sqlx é estrito com o tipo declarado:** `stock` é INTEGER (leia como i64 ou `CAST(... AS REAL)`); ler f64 de
  coluna INTEGER dá erro de decode.
- **Gravações:** sempre `db.begin_write()` (uma por vez no app inteiro); executor `&mut **tx`; não chamar
  `db.pool()` com a transação aberta (no banco em memória o pool tem 1 conexão e trava).
- **Slint:** `for` sobre inteiro calculado quebra o gerador de código (use array fixo); elementos dentro de `if`
  não são acessíveis por id de fora; filhos de `Flickable`/`Rectangle` sem layout precisam de `x`/`y`.
- **Prints das telas sem janela:** exemplos `*_shots` (ver Comandos). Use para conferir cada tela.
- **Windows:** não compila para Windows neste Mac (SQLite embutido e ring/rustls precisam do SDK C do Windows).
  O executável final precisa ser compilado num PC Windows (`cargo build --release -p barerp-app`) ou com
  cargo-xwin (exige aceitar a licença do SDK da Microsoft: decisão do Pablo).
- `rust/target` é compartilhado. **Não crie outro `CARGO_TARGET_DIR`**: cada um passa de 10 GB e já encheu o disco.
  O código gerado das telas tem ~500 mil linhas: `cargo check -p barerp-app` leva ~4 min e o build ~8 min
  (sem debuginfo para esse crate, ver `Cargo.toml`). Sem o cache incremental o build passa de 1 hora, mas o cache
  dele cresce ~4 GB a cada versão das telas: quando o disco apertar, apague `target/debug/incremental/barerp_app-*`.
- **Slint:** `transform-rotation` não vale no elemento raiz de um componente (embrulhe num `Rectangle`).

## Estado (20/10/2026)
As seis áreas do menu original estão portadas, cada uma com o checklist em `barerp-app/paridade/<área>.md`
(o único `[ ]` restante depende do CI publicar o instalador nativo). O `app.slint` só tem a
casca (barra superior, login, troca de senha, bloqueio por licença); as telas antigas feitas às pressas foram apagadas
e o `controller.rs` virou só a casca do app (login, tema, impressão pedida pelos celulares, status do servidor).

- **Mesas & Salão, Caixa & PDV, KDS, Cardápio & Estoque, Módulo Fiscal, Retaguarda:** telas e regras completas.
- **Servidor embutido** na porta 3001 (`main.rs`), com QR de conexão dos celulares, recibos impressos no caixa a
  pedido do celular e `GET /api/settings` + ativação (`/api/settings/license/*`) para os APKs travarem com a licença vencida.
- **Serviços de fundo** (`src/pages/retaguarda.rs`): backup (ao abrir, a cada 6 h e ao fechar o caixa), cardápio digital
  (8 s depois de mudanças no cadastro e a cada 5 min), licença (a cada 60 s; painel do Render a cada 2 min), auto-impressão
  da comanda da cozinha (`kds:new_order`). Agendador de NF-e recebidas no `barerp-server/src/fiscal/agendador.rs`.
- **Impressão:** comprovante, 2ª via, comanda da cozinha, página de teste e DANFE NFC-e em ESC/POS; impressora em branco
  usa a padrão do sistema (Windows ou CUPS), e o campo aceita o nome de uma impressora instalada ou o IP de rede.

## O que falta (em ordem)
1. **CI/release do app nativo** (decisão do Pablo): o atualizador (`barerp-updater`) já usa a mesma release `latest`
   (`version.json` + `BarERP-Instalador-Windows.exe`); o CI precisa gerar o instalador nativo com esse nome e definir
   `BARERP_COMMIT`/`BARERP_BUILD_TIME` na compilação.
2. **Instalador Windows** (feito em 21/10/2026, falta testar num PC Windows):
   - `crates/barerp-app/build.rs` embute o ícone e o VERSIONINFO no `.exe` (embed-resource, só com `target_os=windows`).
   - `.cargo/config.toml` liga o `+crt-static` no alvo `x86_64-pc-windows-msvc`, para o app abrir sem o Visual C++
     Redistributable instalado.
   - `installer/barerp.nsi` (NSIS, pt-BR) gera `target/installer/BarERP-Instalador-Windows.exe`, com dois modos:
     - **Todos os usuários** (a troca): pede administrador, instala em `Arquivos de Programas\BarERP Nativo` (registro em
       HKLM, atalhos públicos) e substitui o Electron. Fecha o `BarERP Pro.exe` e o `barerp.exe`, guarda uma cópia do
       `bar.db*` em `copia-antes-da-troca\` (uma vez só), roda o desinstalador do Electron em silêncio com `--updated`
       (assim ele não apaga o `%APPDATA%`) e só apaga atalhos antigos que apontem para o exe velho.
     - **Só para mim**: sem administrador, em `%LOCALAPPDATA%\Programs\BarERP Nativo`, sem tocar no Electron.
     - Atualizações seguem o modo já instalado (no modo de todos os usuários pedem administrador; se o UAC for recusado,
       avisa e reabre o app). O app é aberto sempre pelo `explorer.exe`, nunca como administrador.
     - Atalho "BarERP Pro" na Área de Trabalho e no Menu Iniciar; a desinstalação não mexe no `%APPDATA%`.
     - Ainda não testado num Windows de verdade (só compila).
   - **Atenção:** o atalho tem o mesmo nome do atalho do Electron. O instalador nativo sobrescreve o do Electron (é o que se
     quer na troca), mas desinstalar o Electron depois apaga o atalho, que agora aponta para o app nativo. Não
     desinstale o Electron no PC do cliente, ou recrie o atalho depois. O desinstalador nativo só apaga o atalho se
     ele apontar para `$INSTDIR\barerp.exe`.
   - Para gerar: `makensis installer/barerp.nsi`, depois do `cargo build --release --target x86_64-pc-windows-msvc`.
     No Windows, basta o NSIS oficial. Neste Mac, o `makensis` foi compilado a partir do código do NSIS 3.11.
   - Falta confirmar no PC do cliente o nome da pasta do `bar.db` (`BarERP Pro`, `bar-erp` ou `BarERP`).
   - O build pelo Mac usa o cargo-xwin, que exige aceitar a licença do SDK da Microsoft.
3. **Conferência no Windows:** impressão real (spooler RAW e rede), ID da máquina igual ao do Electron no PC do JUBAR,
   área de transferência, diálogos de arquivo.
4. **Antes da troca:** testar com uma CÓPIA do bar.db real do JUBAR, conferir os APKs contra o servidor Rust, homologar o
   fiscal na Focus (só com autorização do Pablo), e só então o Pablo decide a data da troca.

## Estado do `barerp-server`
- Pronto: health, network/devices, staff (public-list, login), tables (GET no formato Prisma com `activeOrder`, CRUD, batch/section, sections/rename, open, request-closing, reopen, transfer, merge), orders (GET, itens, 10%, desconto, pay, quick-sale, history, print-receipt), products (GET, categories/all, modifier-groups, promotions, foto), cash (open, shift, close, transactions), kds (queue, item status, ready-all, batch-status), settings (licença, ativação, margem, impressoras), fiscal (`src/fiscal/`), Socket.IO (`devices_updated`, `print-station:register`, `kds:*`), rate limit, CORS, helmet e compressão. API para o app: `ServerHandle` com `emit(&events)`, `set_local_printer(tx)`, `devices()`, `local_ips()` e `port()`.
- Também: waiters, customers, suppliers, staff (CRUD), orders/:id/loss, dashboard, audit-logs, system (backup e cardápio, com os mesmos serviços do app via `set_system_services`), escritas de produtos/categorias/adicionais/promoções/fotos e import-xml. Todas as rotas do Node estão portadas.

---

# PARIDADE TOTAL COM O APP ORIGINAL (pedido do Pablo em 18/10/2026)
"Quero o aplicativo igual ao original, porém em Rust, com todas as funções preservadas e interface bonita."
A versão Rust ainda tem menos funções que o original. Cada área do menu original é portada **por inteiro**
(todas as telas, modais, botões, filtros, atalhos e mensagens do `.tsx`), conferida item por item.

## Como cada área é organizada (para trabalhar em paralelo sem conflito)
- Tela: `barerp-app/ui/pages/<area>.slint`, com `export global <Area>Store` (estado + callbacks) e o
  componente `<Area>AreaPage`. O app.slint só troca a linha `if root.page == Page.<x>: ...` da área.
- Lógica: `barerp-app/src/pages/<area>.rs`, com `pub fn bind(ctx: &AppCtx)`, usando `ui.global::<<Area>Store>()`,
  `ctx.spawn(fut, |ui, resultado| ...)`, `ctx.emit(&events)` e `ctx.status(msg, erro)`. Estado da página num
  `thread_local!` do próprio arquivo.
- Regras de banco: arquivos novos em `barerp-db/src/` da própria área (convenções no topo deste README).
- Checklist: `barerp-app/paridade/<area>.md`, com uma linha por função do original:
  `- [x]` feito, `- [ ]` falta, `- [~]` diferente de propósito (dizendo por quê).
- O código antigo da área em `controller.rs`/`ui/*.slint` (feito às pressas) só é apagado depois que a página
  nova cobre tudo. Quem apaga é o dono do `controller.rs` (sessão principal), a pedido.

## Divisão
| Área (menu original) | Telas do original | Dono |
|---|---|---|
| Mesas & Salão (`mesas`) | TablesView, TableCard, TableDetailsModal, OpenTableModal, AddOrderModal, ModifierPickerModal, TransferModal, MergeModal, CheckoutModal, OrderHistoryModal, ManageTablesModal, ManageWaitersModal, ManageStaffModal, LoginScreen, ConnectMobileModal, Navbar, ThermalReceipt/KitchenTicket (impressão) | sessão principal |
| Caixa & PDV (`caixa`) + KDS (`kds`) | CashView, QuickSaleModal, KdsView, KdsApp | erp-rust-d5 |
| Cardápio & Estoque (`cardapio`) | ProductsView, ModifierGroupsManager, PromotionsManager, StockSheetModal, DigitalMenuItemsModal, ProductPhotoField, NcmLookupModal, SuppliersView, CustomersView | erp-rust-63 |
| Módulo Fiscal (`fiscal`) | FiscalHubView, ManualNfceView, EmitNfeView, FiscalImportView, ImportXmlModal, NfReceivingView, PendingInvoicesView, NfceCancel/Reprint/Inutilizacao, NfeCancel/Reprint/Inutilizacao, AccountantPanelView, FiscalSettingsView, SefazStatusBadge, QrScannerModal | erp-rust-aa |
| Retaguarda (`retaguarda`) | SettingsView, PrinterSettingsView, SubscriptionView, LicenseModal, DashboardView, AuditView, BackupSettingsCard, DigitalMenuCard, ServerConfigModal, UpdateNotificationModal, AppUpdateBanner | erp-rust-d5 (depois do Caixa/KDS) |
