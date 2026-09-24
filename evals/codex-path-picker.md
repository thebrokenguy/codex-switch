# Eval: seleção manual do executável do Codex

## Cenário: instalação fora do caminho padrão

- Dado que a detecção automática não encontra `codex.exe`.
- Quando o usuário abre **Config** e escolhe um arquivo chamado `codex.exe`.
- Então o caminho é validado, salvo nas configurações locais e exibido como **Manual**.
- E o fluxo **Adicionar conta** usa esse executável na próxima tentativa.

## Cenário: voltar para a detecção automática

- Dado que existe um caminho manual salvo.
- Quando o usuário seleciona **Usar detecção automática**.
- Então o caminho manual é removido das configurações.
- E o app volta a procurar nos caminhos padrão, no `PATH` e no override `CODEX_SWITCH_CODEX_BIN`.

## Cenário: caminho manual inválido

- Dado que o usuário seleciona um arquivo inexistente ou diferente de `codex.exe`.
- Então o app rejeita a seleção, mantém a configuração anterior e mostra o erro sem iniciar o login.

## Cenário: executável manual substituído depois da seleção

- Dado que um `codex.exe` válido foi salvo e depois substituído por um arquivo que não é um PE válido.
- Quando o app tenta iniciar um novo login ou reabrir o Codex.
- Então o caminho é revalidado, o arquivo inválido não é executado e o app orienta o usuário a selecionar outro executável.

## Evidências desta avaliação

- Testes Rust cobrem persistência, detecção, revalidação de executável substituído, rejeição de payload que não é PE e limpeza de pastas de login isolado.
- A validação visual confirmou o diálogo nativo, a exibição do caminho como **Manual** e o retorno para **Automático**.
- Nenhum login real foi iniciado durante a avaliação.
