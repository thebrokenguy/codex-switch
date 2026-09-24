# codex-switch: brief do projeto

- Status: v1 concluída em 2026-09-18; seleção manual do executável adicionada e validada em 2026-09-24 (exe portátil buildado; revisão independente e auditoria de publicação realizadas)
- Tipo: pessoal
- Repositório: local, sem remoto por enquanto
- Fonte técnica: este repositório e o estado real da máquina

## Objetivo

Alternar entre contas ChatGPT/Codex na mesma máquina Windows, sem perder nada do contexto local (sessões, memórias, AGENTS.md, skills, plugins, MCPs), com painel de cotas na bandeja e troca em um clique.

## Usuário

Usuário pessoal, somente contas próprias. Cenário típico: a conta A estoura a cota, troca para a B, e volta quando A liberar.

## Como funciona

- Cada conta é um perfil: cópia do `auth.json` guardada em `%USERPROFILE%\.codex\auth-profiles\`.
- Trocar de conta significa substituir apenas o `%USERPROFILE%\.codex\auth.json` pelo do perfil escolhido.
- Nada mais dentro de `%USERPROFILE%\.codex` é alterado: sessões, memórias, AGENTS.md, skills, plugins, `config.toml` e MCPs continuam como estão.
- Conta nova entra por login isolado (`codex login` com `CODEX_HOME` temporário), sem `logout` na conta ativa, para não revogar refresh tokens.
- O binário do Codex é procurado nesta ordem: `CODEX_SWITCH_CODEX_BIN` (override), caminho manual salvo pelo usuário, instalação separada (`%LOCALAPPDATA%\Programs\OpenAI\Codex\bin`), pasta do app desktop (`%LOCALAPPDATA%\OpenAI\Codex\bin`, escolhendo a versão mais recente) e PATH. O diálogo Config permite selecionar `codex.exe` ou voltar à detecção automática.
- Antes de sair de uma conta, o perfil dela é re-salvo a partir do `auth.json` vivo (captura tokens rotacionados), com backup datado e verificação por hash.
- O Codex guarda a credencial em memória, então a troca pede fechar e reabrir o app; o codex-switch detecta, confirma e reabre.

## Cotas

- Leitura a cada X minutos (padrão 5, configurável), consultando o mesmo endpoint usado pelos clientes (`chatgpt.com/backend-api/wham/usage`) com o token de cada conta.
- Colunas: CONTA, PLANO, USO 5H (percentual restante e horário do reset), USO SEMANAL (percentual e dia/hora do reset), ÚLTIMA ATIVIDADE.
- Conta sem token válido: mostra o último valor conhecido, marcado como desatualizado.
- Renovação de token: cada perfil é renovado quando preciso, exceto a conta ativa com o Codex rodando (para não competir com a rotação do próprio Codex).

## Escopo da v1

- Janela painel + ícone na bandeja com menu rápido (abrir painel, atualizar cotas agora, sair).
- Perfis: salvar, trocar, renomear, remover, adicionar conta (login isolado).
- Backup automático antes de cada troca (cópia datada com hash em `_backups\`; restauração manual copiando o arquivo desejado sobre o `auth.json`).
- Fechar e reabrir o Codex na troca, com confirmação.
- Atalho clicável e opção "iniciar com o Windows" (desligada por padrão; via pasta Inicializar, sem registro).
- Exe portátil, sem instalador; configuração ao lado do exe (fallback: `%USERPROFILE%\.codex\auth-profiles\`).
- Seleção visual do `codex.exe` em Config, validação do arquivo e retorno à detecção automática.

## Fora do escopo (v1)

- macOS/Linux, multi-máquina, warm-up agendado, assinatura de código, troca automática sem confirmação.

## Riscos e mitigações

- Rotação de refresh token: re-salvar perfil antes de sair; backup datado; verificação por hash.
- Token vencido em conta parada: exibir dado antigo marcado; renovação automática no polling (fora da conta ativa com Codex aberto) e nova renovação pelo próprio Codex ao trocar para ela.
- Endpoint interno pode mudar: parser isolado com testes e fixtures; degradação graciosa.
- Fechar o Codex pode interromper trabalho: confirmação explícita; reabertura automática.
- Exe sem assinatura: SmartScreen só avisa se o arquivo vier de outro lugar; uso pessoal, sem problema.
- Credenciais em texto plano (mesma sensibilidade do `auth.json`): pasta local, fora de nuvem e de git.
- Crash durante o login isolado pode deixar uma pasta temporária em `%TEMP%`; o app remove sessões ativas ao sair e limpa pastas isoladas antigas na próxima inicialização.

## Critérios de sucesso

- Salvar a conta A, adicionar a conta B e alternar A/B com um clique, sem perder contexto.
- Painel com as 5 colunas usando dados reais, atualizando sozinho no intervalo.
- Nenhuma senha armazenada; nenhum arquivo de credencial persistente fora de `%USERPROFILE%\.codex`. O login isolado usa uma pasta temporária somente durante o fluxo e a remove ao concluir, cancelar, expirar ou reiniciar o app.
- Suíte de testes do núcleo verde (62 testes) e validação manual na máquina real.
- Quando a detecção automática falhar, o usuário consegue escolher e validar o `codex.exe` sem configurar variável de ambiente.

## Decisões tomadas

- Stack: Tauri 2 (núcleo Rust + UI HTML/CSS/TS), exe único portátil.
- Perfis em `%USERPROFILE%\.codex\auth-profiles\`; backups em `_backups\` dentro dessa pasta.
- UI em PT-BR, no estilo da referência (tabela escura, monoespaçada).
- Sem senhas: login sempre no navegador (fluxo oficial).
- Leitura de cota a cada 5 minutos por padrão.

## Referências estudadas

- Loongphy/codex-auth (modelo de lista com cotas)
- ElysiaTT/codexchange (fluxo de perfis e login isolado)
- Lampese/codex-switcher (cuidados com rotação de token; bandeja)
- wikty/codex-accounts (modos de troca; nota sobre janelas)
- ndycode/codex-multi-auth (referência de robustez)

## Próximo passo

- v1 concluída: exe portátil em `%LOCALAPPDATA%\Programs\codex-switch\codex-switch.exe`, atalhos no Desktop e Menu Iniciar, toggle "iniciar com o Windows" no diálogo Config.
- Uso real com as contas próprias (a segunda conta entra pelo botão "Adicionar conta").
- Futuro (v1.1): agendamento por horário de reset.
