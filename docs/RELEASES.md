# Releases e atualização automática

O codex-switch é distribuído como app portátil (exe único) e se atualiza
sozinho a partir das GitHub Releases. Este documento explica o fluxo, como
publicar uma versão e os detalhes de integridade.

## Como funciona

1. Você envia uma tag `vX.Y.Z` (ex.: `v0.2.0`) para o GitHub.
2. O workflow `release.yml` confere se a tag bate com a versão em
   `src-tauri/tauri.conf.json`, compila em modo portátil e publica a Release com:
   - `codex-switch-portable.exe` (o app; roda de qualquer pasta);
   - `update-manifest.json` (versão + SHA-256 lidos pelo auto-update).
3. O app checa o `update-manifest.json` da release mais recente (10s após abrir
   e a cada 6h). Se a versão for maior, baixa o exe, confere o SHA-256 e deixa
   o arquivo staged ao lado do exe atual.
4. Na abertura seguinte o app troca os arquivos (o antigo vira
   `codex-switch.exe.old`) e reabre sozinho já na versão nova.

O workflow `ci.yml` roda build do frontend e `cargo test --all-targets` em todo
PR e em todo push para `main`, antes de qualquer release.

## Publicando uma versão nova

1. Atualize a versão nos três lugares:
   - `src-tauri/tauri.conf.json` (campo `version`, é ele que o auto-update compara);
   - `src-tauri/Cargo.toml`;
   - `package.json`.
2. Commit na `main` e crie a tag:

   ```bash
   git tag v0.2.0
   git push origin main --tags
   ```

3. A release aparece em https://github.com/thebrokenguy/codex-switch/releases
   e os apps em uso atualizam sozinhos.

Se a tag não bater com a versão do app, o workflow falha com uma mensagem
clara e nada é publicado.

## Integridade da atualização

- O exe novo só é aplicado se o SHA-256 bater com o do `update-manifest.json`.
  Qualquer divergência descarta o download e mantém a versão atual; na troca,
  o hash é conferido de novo e uma falha no meio devolve o exe original.
- O manifesto vem da release mais recente do GitHub via HTTPS (mesma origem do
  exe). Isso protege contra corrupção de download; proteger contra publicação
  maliciosa na própria conta do GitHub exigiria assinatura com chave offline.
- Se quiser assinatura no futuro, dá para adicionar verificação minisign em
  `src-tauri/src/core/appupdate.rs` com a chave de
  `C:\Users\vinic\.tauri\codex-switch.key` (o secret `TAURI_SIGNING_PRIVATE_KEY`
  ainda existe no repositório e hoje está sem uso).

## Comportamento no Windows

- Nada é instalado: o exe roda de qualquer pasta, sem admin e sem registro.
- A troca acontece na abertura; o app fecha e reabre sozinho na versão nova.
- O exe antigo fica como `codex-switch.exe.old` (backup) e é limpo nas
  aberturas seguintes.
- Configurações e perfis de conta não são tocados pela atualização.

## Solução de problemas

| Sintoma | Causa provável |
| --- | --- |
| Workflow de release falha na etapa "Confere tag x versão" | Tag diferente da versão em `tauri.conf.json` |
| App nunca atualiza | `update-manifest.json` ausente na release, versão já igual, ou app em modo dev |
| Log com "hash do exe baixado não confere" | Download corrompido ou manifesto errado; a versão atual é mantida |
| App abriu e fechou de novo sozinho | Troca de versão em andamento; é o comportamento esperado |
