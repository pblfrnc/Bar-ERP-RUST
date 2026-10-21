# Paridade: Retaguarda (`retaguarda`)

Original: `SettingsView.tsx`, `DashboardView.tsx`, `AuditView.tsx`, `PrinterSettingsView.tsx`, `SubscriptionView.tsx`,
`LicenseModal.tsx`, `BackupSettingsCard.tsx`, `DigitalMenuCard.tsx`, o aviso de mensalidade e o bloqueio do `App.tsx`,
e as rotas `settings.ts`, `dashboard.ts`, `auditLogs.ts`, `system.ts`.
Port: `ui/pages/retaguarda*.slint` (`RetaguardaStore`) + `src/pages/retaguarda.rs`; banco em `barerp-db/src/dashboard.rs`
e `retaguarda.rs`; licença no crate `barerp-license`; impressão em `src/print.rs`.
Prints: `cargo run -p barerp-app --example retaguarda_shots -- <cópia do banco> <pasta>`.
Legenda: `[x]` feito, `[ ]` falta, `[~]` diferente de propósito (com o motivo).

## Hub (SettingsView)
- [x] Cabeçalho "Retaguarda & Gestão do Sistema" com Modo Claro/Escuro e "Aumentar Tamanho" (1x / 1.2x / 1.4x)
- [x] Cartão "Assinatura & Licença" com dias restantes (âmbar ≤ 5) e "Terminal: <16 primeiros do ID>..."
- [x] Cartão "Métricas & Gestão" (Faturamento & Relatórios) abre o Dashboard
- [x] Margem de Lucro Padrão (1% a 99%) com Salvar e os avisos do original
- [x] Backup Automático (cartão completo, abaixo)
- [x] Cardápio Digital (cartão completo, abaixo)
- [x] Equipe & Colaboradores, Garçons e Conectar Celular abrem as mesmas janelas da área Mesas, por cima da Retaguarda
- [x] Clientes (Fiado) e Fornecedores abrem dentro da Retaguarda (mesmas telas da área Cardápio), com seta de voltar
- [~] Submenu no topo da Retaguarda (Visão Geral, Métricas, Equipe, Garçons, Clientes, Fornecedores, Impressoras,
  Conectar Celular, Assinatura) para ir direto a cada parte; em tela estreita mostra só os ícones (pedido do Pablo, 21/10/2026)
- [~] Clientes (Fiado) saiu das abas da área Cardápio & Estoque (fica só na Retaguarda, como no original)
- [x] Configurar Impressoras & Bobina
- [x] Auto-Impressão de Cozinha (liga/desliga, lembrada neste PC): imprime a comanda dos pedidos novos (`kds:new_order`)
- [~] Auto-impressão sem o "plim" sonoro do navegador (o app nativo não toca som)
- [x] "Bar ERP Pro Desktop" com a versão; selo "Atualização Disponível" e "Atualizar Agora" quando houver
- [~] Verificar Atualizações ("Verificando..."; com novidade abre a janela). O resultado fica na tela com a hora da
  checagem (o original apagava em 6 s e parecia que o botão não fazia nada)

## Atualização do app (updater.cjs + UpdateNotificationModal; crate `barerp-updater`, `src/pages/update.rs`)
- [x] Checagem 15 s depois de abrir e a cada 4 horas, nas três estratégias do Node (version.json da release,
      releases.atom + HEAD do instalador, API do GitHub), mesma release `latest` de `pblfrnc/bar-erp`
- [x] Aviso flutuante "Nova Versão" (Ver Detalhes / Reiniciar Agora / Lembrar mais tarde) e selo na barra superior
- [x] Janela: versão atual, data no GitHub, novidades, download com barra, MB e velocidade, erro, "Reiniciar e
      Atualizar Agora" (abre o instalador desanexado e fecha o app), aviso de dados preservados
- [~] Commit e hora do build vêm de `BARERP_COMMIT`/`BARERP_BUILD_TIME` na compilação (o Electron lia o
      `build-info.json`); o CI precisa definir as duas ao gerar o instalador nativo
- [~] `AppUpdateBanner.tsx` é do APK Android e continua no APK
- [ ] Funciona de verdade só quando o CI publicar o instalador nativo com o mesmo nome (decisão do Pablo)

- [x] Dispositivos Conectados (N) em tempo real (`devices_updated`): App Garçom / Tela Cozinha (KDS), garçom, aparelho e IP

## Backup (BackupSettingsCard + backupService)
- [x] Selo Protegido / Atenção e os avisos: erro, pasta inacessível, sem backup externo, último externo com mais de 24 h
- [x] Último backup no computador (N cópias) e último externo (N cópias na pasta / desligado)
- [x] Pasta externa com "Escolher pasta" (diálogo nativo), chave "Copiar para a pasta externa", "Guardar N dias"
- [x] Salvar (validações do Node: caminho completo, fora da pasta do banco, pasta gravável) e "Fazer backup agora"
- [x] Backup ao abrir (15 s depois, se não houver de hoje), a cada 6 horas e ao fechar o caixa

## Cardápio digital (DigitalMenuCard + digitalMenu)
- [x] Selo No ar / Atenção / Desligado e o erro da última publicação
- [x] QR do link, link, última publicação, Abrir cardápio, Copiar link, "QR Codes das mesas (PDF)"
- [x] Publicar na internet, Escolher itens/fotos/descrições (abre a janela da área Cardápio), nome e recado
- [x] Categorias com clique para esconder/mostrar, Ignorar o estoque, Esconder sem estoque, Avançado (endereço do site)
- [x] Salvar (publica na hora; "Configuração salva, mas não foi possível publicar: ...") e Publicar agora
- [x] Publicação automática 8 s depois de mudanças no cadastro (`catalog:updated`) e conferência a cada 5 min

## Dashboard & Indicadores (DashboardView + dashboard.ts)
- [x] Voltar, título, filtro por data e Atualizar
- [~] Data escolhida com setas (dia anterior/seguinte) e "Hoje" em vez do calendário do navegador; nunca passa de hoje
- [x] Abas Visão Geral / Monitoramento Cego
- [x] Faturamento, Comandas Pagas, Ticket Médio, Taxa de Ocupação (N de M mesas)
- [x] Top 5 produtos com barra, Distribuição dos Pagamentos com %, Vendas por Hora (pico, com valor ao passar o mouse),
      Performance dos Garçons (1º/2º/3º), Alerta de Lucratividade (< 30%)
- [x] Textos "Hoje"/"sáb., 18/10" e os estados vazios do original
- [~] Valores em R$ com vírgula (o original usava `toFixed(2)` com ponto) e formas de pagamento com nome (o original
      mostrava `CASH`, `CREDIT_CARD`)
- [~] Dia LOCAL e datas em ms ou texto entram no mesmo dia (o Node perdia as linhas gravadas em texto)

## Auditoria Cega (AuditView)
- [x] Últimas 100 ações: Data/Hora, Ação (vermelho cancelamento, âmbar desconto), Detalhes; Atualizar; vazio
- [~] Ordenada pelo instante real (no Node o texto antigo aparecia sempre antes dos registros novos)

## Impressoras (PrinterSettingsView)
- [x] Voltar, Restaurar Padrões (com confirmação), Salvar Configurações / Salvo com Sucesso!
- [x] Impressora do caixa e da cozinha com a lista das impressoras instaladas e a padrão marcada (spooler do Windows;
      CUPS no macOS/Linux)
- [x] Vazio = impressora padrão do sistema (caixa) / mesma do caixa (cozinha); o aviso mostra qual é a padrão
- [x] Também aceita o IP de uma impressora de rede (porta 9100)
- [x] Aviso da NF-e A4 (vai pelo PDF da nota, não pela térmica)
- [x] Bobina 80/58 mm (com os ajustes de QR e margem do original) e largura personalizada (40–120)
- [x] Margens (sliders com passo), QR da NFC-e (60–160 + atalhos Micro…Grande), escala do texto (70–115 + atalhos)
- [x] Guilhotina (auto-cut) e impressão silenciosa
- [x] Simulador da bobina em tempo real
- [x] Imprimir Página de Teste (ESC/POS direto, com as configurações da tela, sem salvar)
- [~] Na impressão ESC/POS direta valem largura, corte, avanço e cópias; margens e escala do texto ficam para o
      simulador (a térmica usa a área útil dela)

## Assinatura (SubscriptionView)
- [x] Voltar, "Minha Assinatura & Licença", selo Regularizada, Sincronizar com Servidor (mensagem por 8 s)
- [x] Status da licença, Expira em Breve / Operação Liberada, dias restantes, vencimento por extenso, estabelecimento
- [x] ID da máquina (clique ou botão copia), "Hash Único"
- [x] Renovação antecipada: +1/+3/+6/+12 meses (Popular), projeção da nova validade, PIX com Copiar, WhatsApp com a
      mesma mensagem do original
- [x] Ativação offline (sanfona) com as mensagens do original
- [x] Contato do desenvolvedor e "Falar no WhatsApp"

## Licença (LicenseModal + settings.ts)
- [x] ID da máquina igual ao `node-machine-id` (SHA-256 do MachineGuid do Windows): a licença continua após a troca
- [x] Bloqueio total (inclusive o login) quando não licenciado, expirado, bloqueado ou inválido
- [x] Tela de ativação: Mensalidade Expirada / Licença Bloqueada / Ativação do Bar ERP, ID com Copiar, PIX com Copiar,
      WhatsApp, Verificar Ativação Online, chave offline, endereço do servidor de ativação
- [x] Chave offline `EXP-AAAAMMDD-MAC8-SIG8` e chave legada (e-mail + HMAC), com o mesmo segredo e mensagens
- [x] Revalidação silenciosa no painel a cada 2 min (status a cada 60 s); sem internet mantém a licença local
- [x] Aviso de mensalidade (≤ 5 dias) na barra superior com PIX, ID, WhatsApp e "Acessar Painel de Assinatura"
- [~] Painel fora do ar na ativação mostra o motivo real (o original mostrava "Sua máquina ainda não foi ativada")
- [x] Rotas `GET/PUT /api/settings`, `/license/sync-remote`, `/license/offline` e `/printer` no servidor (os APKs travam com a licença vencida e ativam pela tela deles)
