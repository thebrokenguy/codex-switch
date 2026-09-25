# Releases e atualização automática

O codex-switch se atualiza sozinho a partir das GitHub Releases. Este documento
explica o fluxo, como publicar uma versão e onde ficam as chaves.

## Como funciona

1. Você envia uma tag `vX.Y.Z` (ex.: `v0.2.0`) para o GitHub.
2. O workflow `release.yml` confere se a tag bate com a versão em
   `src-tauri/tauri.conf.json`, compila e publica a Release com:
   - `codex-switch_0.2.0_x64-setup.exe` (instalador NSIS, por usuário, sem admin);
   - `codex-switch_0.2.0_x64-setup.exe.sig` (assinatura da atualização);
   - `latest.json` (manifesto lido pelo updater).
3. O app instalado consulta `latest.json` de tempos em tempos, baixa a nova
   versão, valida a assinatura e instala em modo passivo.

O workflow `ci.yml` roda build do frontend e `cargo test --all-targets` em todo
PR e em todo push para `main`, antes de qualquer release.

## Publicando uma versão nova

1. Atualize a versão nos três lugares:
   - `src-tauri/tauri.conf.json` (campo `version`, é ele que o updater compara);
   - `src-tauri/Cargo.toml`;
   - `package.json`.
2. Commit na `main` e crie a tag:

   ```bash
   git tag v0.2.0
   git push origin main --tags
   ```

3. A release aparece em https://github.com/thebrokenguy/codex-switch/releases
   e os apps instalados atualizam sozinhos.

Se a tag não bater com a versão do app, o workflow falha com uma mensagem
clara e nada é publicado.

## Chave de assinatura (importante)

A atualização só é aceita pelo app se for assinada com a chave privada que
corresponde à `pubkey` em `src-tauri/tauri.conf.json`.

- Chave privada: `C:\Users\vinic\.tauri\codex-switch.key` (gerada em 2026-09-25,
  sem senha de arquivo). **Nunca versionar, nunca enviar em claro.**
- Chave pública: embutida em `src-tauri/tauri.conf.json` (`plugins.updater.pubkey`).

A chave privada precisa existir como secret do repositório GitHub:

```bash
gh secret set TAURI_SIGNING_PRIVATE_KEY --body "$(cat /c/Users/vinic/.tauri/codex-switch.key)"
```

(ou Settings > Secrets and variables > Actions > New repository secret, colando
o conteúdo do arquivo). Opcionalmente `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` se a
chave tiver senha.

Se você perder a chave privada, gere um par novo
(`npx tauri signer generate -w <caminho>`), atualize a `pubkey` no
`tauri.conf.json` e o secret. Apps já instalados vão rejeitar atualizações
assinadas com a chave nova até serem reinstalados uma vez na mão.

## Comportamento no Windows

- Instalação por usuário (`installMode: currentUser`), sem pedir admin.
- A atualização roda em modo passivo (janela de progresso curta).
- Por limitação dos instaladores do Windows, o app encerra antes de aplicar a
  atualização; abra de novo para ver a versão nova.

## Solução de problemas

| Sintoma | Causa provável |
| --- | --- |
| Workflow de release falha na etapa "Confere tag x versão" | Tag diferente da versão em `tauri.conf.json` |
| Build falha pedindo `TAURI_SIGNING_PRIVATE_KEY` | Secret não configurado no repositório |
| App rejeita a atualização com erro de assinatura | `pubkey` no config não corresponde à chave privada usada no CI |
| App nunca atualiza | `latest.json` ausente na release, ou app rodando em modo dev/portátil sem instalador |
