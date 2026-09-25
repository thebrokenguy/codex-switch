# Eval: release e atualização automática

## Cenário: release gerada por tag

- Dado que a versão em `src-tauri/tauri.conf.json` é `0.2.0`.
- Quando o maintainer envia a tag `v0.2.0` para o GitHub.
- Então o workflow `release.yml` compila o app e publica a release `v0.2.0`.
- E a release contém o instalador NSIS, o arquivo `.sig` e o `latest.json`.

## Cenário: tag divergente da versão do app

- Dado que a versão em `src-tauri/tauri.conf.json` é `0.2.0`.
- Quando o maintainer envia a tag `v0.3.0`.
- Então o workflow falha na conferência da tag com mensagem explicando o ajuste.
- E nenhuma release é publicada.

## Cenário: app instalado atualiza sozinho

- Dado que existe uma release `v0.2.0` publicada com `latest.json` válido.
- Quando o app instalado (versão `0.1.0`) roda a checagem automática.
- Então ele baixa a atualização, valida a assinatura com a pubkey embutida e instala em modo passivo.
- E ao abrir de novo, o app reporta a versão `0.2.0`.

## Cenário: falha de rede na checagem

- Dado que o app instalado não alcança o endpoint de atualização.
- Quando a checagem automática roda.
- Então o erro é registrado sem interromper o app.
- E a checagem é repetida no ciclo seguinte.

## Cenário: atualização com assinatura inválida

- Dado que uma release foi assinada com uma chave diferente da `pubkey` do app.
- Quando o app tenta instalar a atualização.
- Então a atualização é rejeitada na validação da assinatura.
- E a versão instalada permanece a mesma.

## Evidências desta avaliação

- `cargo test --all-targets` cobre o núcleo (62 testes) com os plugins de
  updater e process registrados.
- `npm run build` (tsc + vite) cobre o TypeScript do `src/updater.ts`.
- A checagem automática é desativada em modo dev (`import.meta.env.DEV`).
- Falha de atualização nunca derruba o app: o erro vira aviso de log.
