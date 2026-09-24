# codex-switch

Utilitário pessoal para Windows que alterna entre contas ChatGPT/Codex na mesma máquina, sem perder o contexto local (sessões, memórias, AGENTS.md, MCPs, skills).

## O que faz

- Painel de cotas por conta: CONTA, PLANO, USO 5H, USO SEMANAL, ÚLTIMA ATIVIDADE (atualiza a cada X minutos).
- Troca de conta com um clique: backup automático, verificação por hash e reabertura do Codex.
- Adicionar conta via login isolado no navegador, sem derrubar a conta ativa.
- Ícone na bandeja (abrir, atualizar cotas, sair), atalho no Desktop e no Menu Iniciar, e opção de iniciar com o Windows.
- Detecção automática do `codex.exe` com opção de selecionar manualmente outro executável e voltar à detecção automática.
- Link discreto para o perfil público no X: [@vinisn93](https://x.com/vinisn93).

## Como usar (build portátil)

1. Configure `poll_interval_minutes` no diálogo Config se quiser outro intervalo (padrão 5 min). No mesmo diálogo, use **Selecionar codex.exe** se a detecção automática não encontrar o executável; **Usar detecção automática** remove a escolha manual.
2. "Salvar conta atual" guarda a conta logada como um perfil.
3. "Adicionar conta" abre o fluxo de login oficial no navegador; ao concluir, a nova conta vira um perfil.
4. "Trocar" substitui o `auth.json` pelo do perfil escolhido, com backup e fechamento/reabertura automática do Codex.
5. Fechar a janela esconde para a bandeja; o app continua lendo cotas.

O exe é portátil e roda de qualquer pasta. O arquivo de configuração fica ao lado do exe (`codex-switch.settings.json`), incluindo o caminho manual escolhido; se a pasta for somente leitura, cai para `%USERPROFILE%\.codex\auth-profiles\`.

## Estado

v1 concluída em 2026-09-18. Suíte do núcleo: 62 testes verdes (`cargo test --all-targets`). Veja `docs/BRIEF.md` para escopo, decisões e riscos.

## Desenvolvimento

Requisitos: Node.js, Rust (toolchain MSVC) e VS Build Tools com workload C++.

```bash
npm install
npm run tauri dev                      # modo desenvolvimento
npm run tauri build -- --no-bundle     # gera o exe portátil em src-tauri/target/release
```

> Importante: gere o exe pelo CLI do Tauri (`npm run tauri build`). `cargo build --release` sozinho NÃO habilita a feature `custom-protocol` e produz um binário em modo dev que tenta abrir `http://localhost:1420` e falha sem o servidor de desenvolvimento.

Ícone: `app-icon.png` na raiz é a fonte; regenere os demais com `npm run tauri icon app-icon.png`.

## Segurança

- Nenhuma senha é armazenada. O login é sempre feito no navegador, no fluxo oficial da OpenAI.
- Perfis de conta ficam em `%USERPROFILE%\.codex\auth-profiles\` (mesma sensibilidade do `auth.json`; nunca sincronizar em nuvem nem versionar).
- O login isolado usa uma pasta temporária apenas durante a autenticação; ela é removida ao concluir, cancelar, expirar ou iniciar novamente o app após uma interrupção.
- O núcleo não faz chamadas de rede próprias além da leitura de cota com o token da própria conta.
