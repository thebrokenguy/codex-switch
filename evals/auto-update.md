# Eval: release e atualização automática (app portátil)

## Cenário: release gerada por tag

- Dado que a versão em `src-tauri/tauri.conf.json` é `0.2.0`.
- Quando o maintainer envia a tag `v0.2.0` para o GitHub.
- Então o workflow `release.yml` compila o app em modo portátil e publica a release `v0.2.0`.
- E a release contém `codex-switch-portable.exe` e `update-manifest.json` com versão e SHA-256.

## Cenário: tag divergente da versão do app

- Dado que a versão em `src-tauri/tauri.conf.json` é `0.2.0`.
- Quando o maintainer envia a tag `v0.3.0`.
- Então o workflow falha na conferência da tag com mensagem explicando o ajuste.
- E nenhuma release é publicada.

## Cenário: app portátil atualiza sozinho

- Dado que existe uma release `v0.2.1` publicada com `update-manifest.json` válido.
- Quando o app portátil (versão `0.2.0`) roda a checagem automática.
- Então ele baixa o exe novo, confere o SHA-256 com o manifesto e deixa o arquivo staged ao lado do exe.
- E avisa que a atualização vale na próxima abertura.
- E na abertura seguinte o exe é trocado (o antigo vira `codex-switch.exe.old`) e o app reabre já na versão `0.2.1`.

## Cenário: falha de rede na checagem

- Dado que o app portátil não alcança a release no GitHub.
- Quando a checagem automática roda.
- Então o erro é registrado sem interromper o app.
- E a checagem é repetida no ciclo seguinte.

## Cenário: download com hash divergente

- Dado que o exe publicado não confere com o SHA-256 do manifesto (ou o download corrompeu).
- Quando o app tenta baixar a atualização.
- Então o download é descartado e nada é staged.
- E a versão instalada permanece a mesma.

## Cenário: troca interrompida preserva o exe original

- Dado que um exe staged tem hash válido.
- Quando a troca falha depois de guardar o exe atual.
- Então o app devolve o exe original para o lugar e registra o erro.
- E o app continua abrindo na versão atual.

## Cenário: modo dev não atualiza

- Dado que o app roda em modo desenvolvimento.
- Quando a checagem automática dispara.
- Então nada é baixado nem trocado.

## Evidências desta avaliação

- `cargo test --all-targets`: 78 testes verdes, incluindo 11 do
  `core/appupdate.rs` (versões, manifesto, SHA-256, staging, rollback e no-op).
- `npm run build` (tsc + vite) cobre o TypeScript do `src/updater.ts`.
- Checagem desativada em modo dev (`import.meta.env.DEV` e `cfg!(debug_assertions)`).
- Falha de atualização nunca derruba o app: o erro vira log e a checagem
  repete no ciclo seguinte.
