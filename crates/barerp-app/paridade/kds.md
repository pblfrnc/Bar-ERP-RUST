# Paridade: KDS (`kds`)

Original: `client/src/views/KdsView.tsx` (344 linhas) + contador da aba em `App.tsx`/`Navbar.tsx`.
Port: `ui/pages/kds.slint` (`KdsStore`, `KdsAreaPage`) + `src/pages/kds.rs`; banco em `barerp-db/src/kds.rs`.
Prints: `cargo run -p barerp-app --example kds_shots -- <pasta>` (fila inventada, temas escuro e claro).
Legenda: `[x]` feito, `[ ]` falta, `[~]` diferente de propósito (com o motivo).

## Cabeçalho
- [x] Ícone ChefHat, título "KDS • Fila de Produção" e selo "N pedidos" (itens na fila da praça escolhida)
- [x] Subtítulo "Atualização em tempo real para equipe de Bar e Cozinha"
- [x] Seletor de estação: "Todas as Estações", "Barman / Bebidas" (Beer), "Cozinha / Petiscos" (ChefHat)
- [x] Botão Wi-Fi: "Wi-Fi OK" com ponto pulsando e QR quando conectado; "Conectar (QR/IP)" vermelho pulsando quando não
- [~] O botão Wi-Fi abre o QR de conexão dos celulares (`MesasStore.open-connect-mobile`): no PC do caixa o
      servidor é o próprio app, então não há IP de servidor para configurar como no `ServerConfigModal` dos APKs
- [x] Botão Recarregar com o ícone girando enquanto carrega

## Fila
- [x] Itens agrupados por comanda (um ticket por mesa), do mais antigo para o mais novo (FIFO)
- [x] Grade 1 / 2 / 3 / 4 colunas conforme a largura (md / lg / xl do original)
- [x] Estado vazio "Tudo limpo! Sem pedidos na fila" + "Novos pedidos lançados pelos garçons aparecerão aqui instantaneamente."
- [x] Cabeçalho do ticket: nome da mesa ou "Mesa N", "CMD #N • cliente" ("Sem nome")
- [~] Comanda sem mesa aparece como "Balcão" (o original mostrava "Mesa Balcão")
- [x] Tempo de espera em minutos desde o item mais antigo: normal, âmbar a partir de 8 min, vermelho pulsando a partir de 15 min
- [x] Item: "Nx", nome do produto, "🍺 Bar" / "🍳 Cozinha"; fundo laranja em preparo e verde quando pronto
- [x] Observação em destaque "⚠️ Obs: …" (adicionais + observação do garçom, como a comanda da cozinha)
- [x] Ações: "Iniciar Preparo" (fila → preparo), "Marcar Pronto!" (preparo → pronto, avisa o garçom), "Marcar Entregue"
- [x] Lista de itens do ticket rola a partir de ~384 px (max-h-96)
- [x] Rodapé "TUDO PRONTO NESTA MESA" (na praça escolhida; em "Todas" vale para todas as praças)
- [x] Erros mostrados num aviso ("Erro ao atualizar status", "Erro ao marcar todos como prontos")

## Atualização
- [x] Recarrega ao entrar na tela e a cada 15 s (polling de contingência do original)
- [x] Recarrega na hora com os eventos `kds:*` e `order:*` (celulares e outras telas)
- [x] Mudanças feitas aqui são emitidas no Socket.IO (garçom recebe o "pronto", outras telas KDS atualizam)
- [x] Contador da aba "KDS (Cozinha)" com todas as praças, atualizado pelos eventos mesmo fora da tela
- [x] Atalho F3 abre o KDS (extra do app nativo)
