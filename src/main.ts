import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import semTokensUrl from "./assets/sem-tokens.png";

// ---------------------------------------------------------------------------
// Tipos (espelham o serde do backend)
// ---------------------------------------------------------------------------

interface UsageView {
  session_text: string;
  weekly_text: string;
  session_remaining: number | null;
  weekly_remaining: number | null;
  error: string | null;
  fetched_at: number | null;
}

interface AccountRow {
  slug: string;
  display_name: string;
  email: string | null;
  plan: string | null;
  is_active: boolean;
  has_file: boolean;
  last_used_at: string | null;
  usage: UsageView;
}

interface ProcInfo {
  ProcessId: number;
  Name: string;
  ExecutablePath?: string | null;
}

type SwitchReport =
  | { status: "needs_confirmation"; processes: ProcInfo[] }
  | {
      status: "switched";
      switched: boolean;
      closed: number;
      reopened: boolean;
      reopen_error: string | null;
      backup: string | null;
      synced: string | null;
    };

type LoginStatus =
  | { state: "idle" }
  | { state: "running" }
  | { state: "done"; profile: { display_name: string; email?: string | null } }
  | { state: "failed"; message: string };

interface Settings {
  poll_interval_minutes: number;
  reopen_after_switch: boolean;
}

interface CodexPathView {
  configured_path: string | null;
  detected_path: string | null;
}

interface IdentityView {
  email: string | null;
  plan: string | null;
}

// ---------------------------------------------------------------------------
// Helpers de DOM
// ---------------------------------------------------------------------------

const $ = <T extends HTMLElement>(sel: string) => document.querySelector(sel) as T;

const rowsEl = $("#rows");
const tableWrap = $("#table-wrap");
const emptyEl = $("#empty");
const codexStatusEl = $("#codex-status");
const lastRefreshEl = $("#last-refresh");
const accountsCountEl = $("#accounts-count");
const liveTag = $("#live-tag");

function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] as string,
  );
}

// Nota de segurança: todo conteúdo dinâmico (nomes, e-mails, mensagens de erro)
// passa por esc() antes de entrar em innerHTML. Slugs vêm do slugify do backend,
// que só produz [a-z0-9-]. Não interpolar string dinâmica sem esc().

let toastId = 0;
function toast(message: string, kind: "ok" | "warn" | "err" | "" = "", ms = 4200) {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  el.dataset.id = String(++toastId);
  $("#toast-root").appendChild(el);
  setTimeout(() => el.remove(), ms);
}

function closeDialog() {
  $("#dialog-root").innerHTML = "";
}

function openDialog(html: string): HTMLElement {
  const root = $("#dialog-root");
  root.innerHTML = `<div class="overlay"><div class="dialog">${html}</div></div>`;
  const overlay = root.querySelector(".overlay") as HTMLElement;
  overlay.addEventListener("mousedown", (e) => {
    if (e.target === overlay) closeDialog();
  });
  return root.querySelector(".dialog") as HTMLElement;
}

interface Choice {
  label: string;
  detail?: string;
  kind?: "" | "primary" | "danger";
  onClick: () => void;
}

function choicesDialog(title: string, bodyHtml: string, choices: Choice[]) {
  const html: string[] = [`<h2>${esc(title)}</h2>`, bodyHtml, `<div class="choices"></div>`];
  const dialog = openDialog(html.join(""));
  const wrap = dialog.querySelector(".choices") as HTMLElement;
  for (const c of choices) {
    const btn = document.createElement("button");
    btn.className = `choice ${c.kind ?? ""}`;
    btn.innerHTML = `${esc(c.label)}${c.detail ? `<small>${esc(c.detail)}</small>` : ""}`;
    btn.addEventListener("click", () => {
      closeDialog();
      c.onClick();
    });
    wrap.appendChild(btn);
  }
}

// ---------------------------------------------------------------------------
// Formatação
// ---------------------------------------------------------------------------

function relTime(isoish: string | null, epoch?: number | null): string {
  const ms = epoch ? epoch * 1000 : isoish ? Date.parse(isoish) : NaN;
  if (Number.isNaN(ms)) return "–";
  const diff = Math.max(0, Date.now() - ms);
  const min = Math.floor(diff / 60000);
  if (min < 1) return "agora";
  if (min < 60) return `há ${min}min`;
  const h = Math.floor(min / 60);
  if (h < 24) return `há ${h}h`;
  const d = Math.floor(h / 24);
  if (d < 30) return `há ${d}d`;
  return new Date(ms).toLocaleDateString("pt-BR");
}

function usageCell(view: UsageView, kind: "session" | "weekly"): string {
  if (view.error) {
    return `<div class="cell ${kind} usage error" title="${esc(view.error)}">indisponível</div>`;
  }
  const text = kind === "session" ? view.session_text : view.weekly_text;
  const remaining = kind === "session" ? view.session_remaining : view.weekly_remaining;
  let cls = "usage";
  if (remaining !== null && remaining !== undefined) {
    if (remaining <= 5) cls += " critical";
    else if (remaining <= 25) cls += " low";
  }
  // Personagem (sem-tokens) apenas na coluna 5H (session):
  // sem número -> personagem; esgotada (0%) -> personagem + 0%; weekly: só números.
  const allowChar = kind === "session";
  const m = text.match(/^(\d+)%(.*)$/);
  let inner: string;
  if (m) {
    const reset = m[2] ? `<span class="reset">${esc(m[2])}</span>` : "";
    if (allowChar && Number(m[1]) === 0) {
      inner = `<img class="sem-tokens" src="${semTokensUrl}" alt="acabou os tokens" title="acabou os tokens" /><span>${m[1]}%</span>${reset}`;
    } else {
      inner = `<span>${m[1]}%</span>${reset}`;
    }
  } else if (allowChar && (text === "–" || text === "-" || text.trim() === "")) {
    inner = `<img class="sem-tokens" src="${semTokensUrl}" alt="acabou os tokens" title="acabou os tokens" />`;
  } else {
    inner = `<span class="reset">${esc(text)}</span>`;
  }
  return `<div class="cell ${kind} ${cls}">${inner}</div>`;
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

let currentRows: AccountRow[] = [];

function renderRows(rows: AccountRow[]) {
  currentRows = rows;
  rowsEl.innerHTML = "";

  const hasAny = rows.length > 0;
  tableWrap.hidden = !hasAny;
  emptyEl.hidden = hasAny;
  accountsCountEl.textContent = hasAny
    ? `${rows.length} conta${rows.length > 1 ? "s" : ""}`
    : "";

  for (const row of rows) {
    const div = document.createElement("div");
    div.className = `row account-row${row.is_active ? " active" : ""}`;
    div.dataset.slug = row.slug;

    const accountCell = `
      <div class="cell col-account">
        <div class="name">${row.is_active ? '<span class="active-dot"></span>' : ""}${esc(row.display_name)}</div>
        ${row.email ? `<div class="email">${esc(row.email)}</div>` : ""}
      </div>`;

    const planCell = `<div class="cell col-plan">${esc(row.plan ?? "–")}</div>`;
    const activity = row.is_active
      ? `<div class="cell col-activity activity">em uso</div>`
      : `<div class="cell col-activity activity">${relTime(row.last_used_at)}</div>`;

    const actions = `
      <div class="cell col-actions">
        <div class="row-actions">
          ${
            row.is_active
              ? ""
              : `<button class="mini use icon" data-act="use" data-slug="${esc(row.slug)}" title="Trocar para esta conta" aria-label="Trocar para esta conta"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M16 3l4 4-4 4" /><path d="M20 7H4" /><path d="M8 21l-4-4 4-4" /><path d="M4 17h16" /></svg></button>`
          }
          <button class="mini icon" data-act="rename" data-slug="${esc(row.slug)}" title="Renomear" aria-label="Renomear"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" /><path d="m15 5 4 4" /></svg></button>
          <button class="mini icon" data-act="remove" data-slug="${esc(row.slug)}" title="Remover" aria-label="Remover"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M18 6 6 18" /><path d="m6 6 12 12" /></svg></button>
        </div>
      </div>`;

    div.innerHTML =
      accountCell + planCell + usageCell(row.usage, "session") + usageCell(row.usage, "weekly") + activity + actions;

    div.addEventListener("click", (e) => {
      const btn = (e.target as HTMLElement).closest("button[data-act]") as HTMLButtonElement | null;
      if (btn) {
        e.stopPropagation();
        const slug = btn.dataset.slug as string;
        if (btn.dataset.act === "use") void doSwitch(slug);
        if (btn.dataset.act === "rename") void doRename(slug);
        if (btn.dataset.act === "remove") void doRemove(slug);
        return;
      }
      if (!row.is_active) void doSwitch(row.slug);
    });

    rowsEl.appendChild(div);
  }
}

async function reload() {
  const rows = await invoke<AccountRow[]>("list_accounts");
  renderRows(rows);
}

async function refreshAll(busyEl?: HTMLElement) {
  busyEl?.classList.add("busy");
  try {
    const rows = await invoke<AccountRow[]>("refresh_usage");
    renderRows(rows);
    lastRefreshEl.textContent = `cotas lidas ${new Date().toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })}`;
  } catch (e) {
    toast(String(e), "err");
  } finally {
    busyEl?.classList.remove("busy");
  }
}

async function updateCodexStatus() {
  try {
    const procs = await invoke<ProcInfo[]>("codex_processes");
    if (procs.length === 0) {
      codexStatusEl.textContent = "Codex: fechado";
      codexStatusEl.className = "status ok";
    } else {
      const names = [...new Set(procs.map((p) => p.Name))];
      codexStatusEl.textContent = `Codex: aberto (${names.join(", ")})`;
      codexStatusEl.className = "status warn";
    }
  } catch {
    codexStatusEl.textContent = "";
  }
}

// ---------------------------------------------------------------------------
// Ações
// ---------------------------------------------------------------------------

function procSummary(procs: ProcInfo[]): string {
  const counts = new Map<string, number>();
  for (const p of procs) counts.set(p.Name, (counts.get(p.Name) ?? 0) + 1);
  return [...counts.entries()].map(([n, c]) => `${n} (${c})`).join(", ");
}

async function doSwitch(slug: string) {
  const row = currentRows.find((r) => r.slug === slug);
  const name = row?.display_name ?? slug;
  try {
    const report = await invoke<SwitchReport>("switch_account", { slug, closeCodex: false });
    if (report.status === "needs_confirmation") {
      const list = procSummary(report.processes);
      choicesDialog(
        "O Codex está aberto",
        `<p>Há processos do Codex em execução: <b>${esc(list)}</b>.</p>
         <p>Para trocar a conta com segurança, eles precisam ser fechados (o app regrava o auth.json ao renovar o login).</p>
         <p>Posso fechar, trocar a conta e reabrir o app do Codex.</p>`,
        [
          {
            label: "Fechar, trocar e reabrir",
            kind: "primary",
            onClick: () => void finishSwitch(slug, name),
          },
          { label: "Cancelar", onClick: () => {} },
        ],
      );
      return;
    }
    handleSwitchDone(report, name);
  } catch (e) {
    toast(String(e), "err");
  }
}

async function finishSwitch(slug: string, name: string) {
  try {
    const report = await invoke<SwitchReport>("switch_account", { slug, closeCodex: true });
    if (report.status === "switched") handleSwitchDone(report, name);
  } catch (e) {
    toast(String(e), "err");
  }
}

function handleSwitchDone(
  report: Extract<SwitchReport, { status: "switched" }>,
  name: string,
) {
  if (!report.switched) {
    toast(`"${name}" já é a conta ativa (tokens sincronizados).`, "ok");
  } else {
    let msg = `Conta ativa agora: ${name}.`;
    if (report.reopened) msg += " App do Codex reaberto.";
    if (report.reopen_error) msg += ` Atenção: ${report.reopen_error}`;
    toast(msg, report.reopen_error ? "warn" : "ok");
  }
  void reload();
  void updateCodexStatus();
  void refreshAll();
}

function doRename(slug: string) {
  const row = currentRows.find((r) => r.slug === slug);
  const dialog = openDialog(`
    <h2>Renomear conta</h2>
    <p>Novo nome para <b>${esc(row?.display_name ?? slug)}</b> (o arquivo interno não muda):</p>
    <input type="text" id="rename-input" value="${esc(row?.display_name ?? "")}" />
    <div class="dialog-actions">
      <button class="btn" data-x="cancel">Cancelar</button>
      <button class="btn primary" data-x="ok">Salvar</button>
    </div>`);
  const input = dialog.querySelector("#rename-input") as HTMLInputElement;
  input.focus();
  input.select();
  const submit = async () => {
    const newName = input.value.trim();
    if (!newName) return;
    closeDialog();
    try {
      await invoke("rename_account", { slug, newName });
      toast("Nome atualizado.", "ok");
      void reload();
    } catch (e) {
      toast(String(e), "err");
    }
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void submit();
  });
  dialog.querySelector('[data-x="cancel"]')!.addEventListener("click", closeDialog);
  dialog.querySelector('[data-x="ok"]')!.addEventListener("click", () => void submit());
}

function doRemove(slug: string) {
  const row = currentRows.find((r) => r.slug === slug);
  choicesDialog(
    "Remover conta",
    `<p>Remover <b>${esc(row?.display_name ?? slug)}</b>?</p>
     <p>Uma cópia de segurança do arquivo fica guardada na pasta <b>_backups</b>. A conta continua logada na OpenAI; isso apaga só o perfil local.</p>`,
    [
      {
        label: "Remover",
        kind: "danger",
        onClick: async () => {
          try {
            await invoke("remove_account", { slug });
            toast("Conta removida (cópia em _backups).", "ok");
            void reload();
          } catch (e) {
            toast(String(e), "err");
          }
        },
      },
      { label: "Cancelar", onClick: () => {} },
    ],
  );
}

// ----- salvar conta atual -------------------------------------------------

async function doSaveCurrent() {
  let suggested = "";
  try {
    const live = await invoke<IdentityView | null>("live_identity");
    if (live?.email) suggested = live.email;
  } catch {
    /* segue com sugestão vazia */
  }
  const dialog = openDialog(`
    <h2>Salvar conta atual</h2>
    <p>Guarda o login que está ativo agora neste PC como um perfil do codex-switch.</p>
    <input type="text" id="save-input" placeholder="ex.: pessoal" value="${esc(suggested)}" />
    <div class="dialog-actions">
      <button class="btn" data-x="cancel">Cancelar</button>
      <button class="btn primary" data-x="ok">Salvar</button>
    </div>`);
  const input = dialog.querySelector("#save-input") as HTMLInputElement;
  input.focus();
  input.select();

  const attempt = async () => {
    const name = input.value.trim();
    if (!name) return;
    try {
      const meta = await invoke<{ display_name: string }>("save_current", {
        name,
        overwrite: false,
      });
      closeDialog();
      toast(`Conta salva: ${meta.display_name}.`, "ok");
      void reload();
      void refreshAll();
    } catch (e) {
      const msg = String(e);
      if (msg.includes("já existe")) {
        choicesDialog(
          "Já existe uma conta com esse nome",
          `<p>${esc(msg)}.</p><p>Quer sobrescrever o perfil existente com o login atual?</p>`,
          [
            {
              label: "Sobrescrever",
              kind: "danger",
              onClick: async () => {
                try {
                  await invoke("save_current", { name, overwrite: true });
                  toast(`Conta salva: ${name}.`, "ok");
                  void reload();
                  void refreshAll();
                } catch (e2) {
                  toast(String(e2), "err");
                }
              },
            },
            { label: "Cancelar", onClick: () => {} },
          ],
        );
      } else {
        toast(msg, "err");
      }
    }
  };

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") void attempt();
  });
  dialog.querySelector('[data-x="cancel"]')!.addEventListener("click", closeDialog);
  dialog.querySelector('[data-x="ok"]')!.addEventListener("click", () => void attempt());
}

// ----- adicionar conta (login isolado) -----------------------------------

let loginPoll: number | undefined;

async function doAddAccount() {
  const dialog = openDialog(`
    <h2>Adicionar conta</h2>
    <p>Vai abrir o <b>login oficial da OpenAI no seu navegador</b>, em área isolada: este fluxo não mexe na conta que está ativa agora.</p>
    <p>Entre com a <b>outra conta</b> e digite a senha lá, na página da OpenAI. A senha nunca passa por este app; só o token de sessão volta e vira um perfil local.</p>
    <input type="text" id="add-input" placeholder="nome da conta (ex.: trabalho)" />
    <div id="add-status"></div>
    <div class="dialog-actions">
      <button class="btn" data-x="cancel">Cancelar</button>
      <button class="btn primary" data-x="ok">Abrir login no navegador</button>
    </div>`);
  const input = dialog.querySelector("#add-input") as HTMLInputElement;
  const statusEl = dialog.querySelector("#add-status") as HTMLElement;
  const okBtn = dialog.querySelector('[data-x="ok"]') as HTMLButtonElement;
  input.focus();

  const stopPolling = () => {
    if (loginPoll !== undefined) {
      window.clearInterval(loginPoll);
      loginPoll = undefined;
    }
  };

  const onDone = () => {
    stopPolling();
    closeDialog();
    toast("Conta adicionada.", "ok");
    void reload();
    void refreshAll();
  };

  const poll = async () => {
    try {
      const st = await invoke<LoginStatus>("add_account_status");
      if (st.state === "done") {
        statusEl.innerHTML = `<div class="ok-box">Login concluído: <b>${esc(st.profile.display_name)}</b>${st.profile.email ? ` (${esc(st.profile.email)})` : ""}. Salvando…</div>`;
        setTimeout(onDone, 600);
      } else if (st.state === "failed") {
        stopPolling();
        okBtn.disabled = false;
        statusEl.innerHTML = `<div class="error-box">${esc(st.message)}</div>`;
      } else if (st.state === "idle") {
        stopPolling();
      }
    } catch (e) {
      stopPolling();
      statusEl.innerHTML = `<div class="error-box">${esc(String(e))}</div>`;
    }
  };

  const start = async () => {
    const name = input.value.trim();
    if (!name) {
      input.focus();
      return;
    }
    okBtn.disabled = true;
    try {
      await invoke("add_account_start", { name });
      statusEl.innerHTML = `
        <div class="spinner-row">
          <div class="spinner"></div>
          <span>Aguardando você concluir o login no navegador… (a janela fecha sozinha ao terminar)</span>
        </div>`;
      loginPoll = window.setInterval(() => void poll(), 1500);
    } catch (e) {
      const message = String(e);
      okBtn.disabled = false;
      if (message.includes("não encontrei o executável do codex")) {
        statusEl.innerHTML = `
          <div class="error-box">
            ${esc(message)}
            <button class="btn" data-x="choose-codex-error">Selecionar codex.exe</button>
          </div>`;
        const chooseButton = statusEl.querySelector('[data-x="choose-codex-error"]') as HTMLButtonElement;
        chooseButton.addEventListener("click", async () => {
          chooseButton.disabled = true;
          try {
            const selected = await chooseCodexBin();
            if (selected) {
              await start();
            } else {
              okBtn.disabled = false;
            }
          } catch (e2) {
            okBtn.disabled = false;
            statusEl.innerHTML = `<div class="error-box">${esc(String(e2))}</div>`;
          }
        });
      } else {
        statusEl.innerHTML = `<div class="error-box">${esc(message)}</div>`;
      }
    }
  };

  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !okBtn.disabled) void start();
  });
  dialog.querySelector('[data-x="cancel"]')!.addEventListener("click", () => {
    stopPolling();
    void invoke("add_account_cancel").catch(() => {});
    closeDialog();
  });
  okBtn.addEventListener("click", () => void start());
}

function codexPathLabel(view: CodexPathView): string {
  if (view.configured_path) {
    if (view.detected_path === view.configured_path) {
      return `Manual: ${view.configured_path}`;
    }
    if (view.detected_path) {
      return `Manual indisponível. Automático: ${view.detected_path}`;
    }
    return `Manual indisponível: ${view.configured_path}`;
  }
  return view.detected_path ? `Automático: ${view.detected_path}` : "Não encontrado";
}

async function chooseCodexBin(): Promise<string | null> {
  const selected = await openFile({
    title: "Selecionar executável do Codex",
    multiple: false,
    directory: false,
    filters: [{ name: "Codex CLI", extensions: ["exe"] }],
  });
  const path = Array.isArray(selected) ? selected[0] : selected;
  if (!path) return null;
  await invoke("set_codex_bin_path", { path });
  return path;
}

// ----- configurações ------------------------------------------------------

async function doSettings() {
  let settings: Settings = { poll_interval_minutes: 5, reopen_after_switch: true };
  try {
    settings = await invoke<Settings>("get_settings");
  } catch {
    /* usa padrão */
  }
  let codexPath: CodexPathView = { configured_path: null, detected_path: null };
  try {
    codexPath = await invoke<CodexPathView>("get_codex_path");
  } catch {
    /* mostra estado desconhecido */
  }
  let startWithWindows = false;
  try {
    startWithWindows = await invoke<boolean>("get_start_with_windows");
  } catch {
    /* assume desligado */
  }
  const dialog = openDialog(`
    <h2>Configurações</h2>
    <p>Intervalo de leitura das cotas (minutos, 1 a 120):</p>
    <input type="number" id="cfg-interval" min="1" max="120" value="${settings.poll_interval_minutes}" />
    <p class="settings-label">Executável do Codex:</p>
    <div class="path-setting">
      <code id="cfg-codex-path">${esc(codexPathLabel(codexPath))}</code>
      <div class="path-actions">
        <button class="btn" data-x="choose-codex">Selecionar codex.exe</button>
        <button class="btn" data-x="auto-codex">Usar detecção automática</button>
      </div>
    </div>
    <p class="settings-hint">A detecção automática continua sendo o padrão. Um caminho escolhido manualmente só é usado quando o arquivo existe.</p>
    <p style="margin-top:14px;">
      <label style="display:flex;gap:8px;align-items:center;cursor:pointer;">
        <input type="checkbox" id="cfg-reopen" ${settings.reopen_after_switch ? "checked" : ""} />
        Reabrir o app do Codex automaticamente após trocar de conta
      </label>
    </p>
    <p style="margin-top:10px;">
      <label style="display:flex;gap:8px;align-items:center;cursor:pointer;">
        <input type="checkbox" id="cfg-startup" ${startWithWindows ? "checked" : ""} />
        Iniciar com o Windows (atalho na pasta Inicializar)
      </label>
    </p>
    <div class="dialog-actions">
      <button class="btn left" data-x="folder">Abrir pasta de perfis</button>
      <button class="btn danger" data-x="quit">Sair do app</button>
      <button class="btn" data-x="cancel">Cancelar</button>
      <button class="btn primary" data-x="ok">Salvar</button>
    </div>`);

  const pathEl = dialog.querySelector("#cfg-codex-path") as HTMLElement;
  const chooseButton = dialog.querySelector('[data-x="choose-codex"]') as HTMLButtonElement;
  chooseButton.addEventListener("click", async () => {
    chooseButton.disabled = true;
    try {
      const selected = await chooseCodexBin();
      if (selected) {
        pathEl.textContent = `Manual: ${selected}`;
        toast("Caminho do Codex salvo.", "ok");
      }
    } catch (e) {
      toast(String(e), "err");
    } finally {
      chooseButton.disabled = false;
    }
  });
  dialog.querySelector('[data-x="auto-codex"]')!.addEventListener("click", async () => {
    try {
      await invoke("set_codex_bin_path", { path: null });
      const latest = await invoke<CodexPathView>("get_codex_path");
      pathEl.textContent = codexPathLabel(latest);
      toast("Detecção automática reativada.", "ok");
    } catch (e) {
      toast(String(e), "err");
    }
  });
  dialog.querySelector('[data-x="folder"]')!.addEventListener("click", () => {
    void invoke("open_profiles_folder").catch((e) => toast(String(e), "err"));
  });
  dialog.querySelector('[data-x="quit"]')!.addEventListener("click", () => {
    void invoke("quit_app").catch(() => {});
  });
  dialog.querySelector('[data-x="cancel"]')!.addEventListener("click", closeDialog);
  dialog.querySelector('[data-x="ok"]')!.addEventListener("click", async () => {
    const minutes = Number((dialog.querySelector("#cfg-interval") as HTMLInputElement).value) || 5;
    const reopen = (dialog.querySelector("#cfg-reopen") as HTMLInputElement).checked;
    const startup = (dialog.querySelector("#cfg-startup") as HTMLInputElement).checked;
    try {
      await invoke("set_settings", { intervalMinutes: minutes, reopenAfterSwitch: reopen });
      if (startup !== startWithWindows) {
        await invoke("set_start_with_windows", { enabled: startup });
      }
      closeDialog();
      toast("Configurações salvas.", "ok");
    } catch (e) {
      toast(String(e), "err");
    }
  });
}

// ---------------------------------------------------------------------------
// Colunas redimensionáveis
// ---------------------------------------------------------------------------

const COLUMN_DEFS: ReadonlyArray<{ key: string; min: number }> = [
  { key: "account", min: 120 },
  { key: "plan", min: 52 },
  { key: "session", min: 90 },
  { key: "weekly", min: 110 },
  { key: "activity", min: 80 },
];

const COLUMN_STORAGE_KEY = "codex-switch.column-widths.v1";

function columnVar(key: string): string {
  return `--col-${key}`;
}

function readStoredColumnWidths(): Record<string, number> {
  try {
    const raw = localStorage.getItem(COLUMN_STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object") return {};
    const out: Record<string, number> = {};
    for (const def of COLUMN_DEFS) {
      const value = (parsed as Record<string, unknown>)[def.key];
      if (typeof value === "number" && Number.isFinite(value) && value >= def.min) {
        out[def.key] = Math.round(value);
      }
    }
    return out;
  } catch {
    return {};
  }
}

function persistColumnWidths() {
  const out: Record<string, number> = {};
  for (const def of COLUMN_DEFS) {
    const raw = tableWrap.style.getPropertyValue(columnVar(def.key)).trim();
    const match = /^(\d+(?:\.\d+)?)px$/.exec(raw);
    if (match) out[def.key] = Math.round(Number(match[1]));
  }
  try {
    if (Object.keys(out).length) {
      localStorage.setItem(COLUMN_STORAGE_KEY, JSON.stringify(out));
    } else {
      localStorage.removeItem(COLUMN_STORAGE_KEY);
    }
  } catch {
    /* armazenamento indisponível: segue sem persistir */
  }
}

function maxColumnWidth(startWidth: number): number {
  // Limite do comprimento: impede que o arrasto estoure a largura da tabela
  // (sem isso, alargar demais uma coluna criaria barra de rolagem).
  const actions = document.querySelector<HTMLElement>(".header-row .col-actions");
  const actionsWidth = actions ? actions.getBoundingClientRect().width : 112;
  const room = tableWrap.clientWidth - tableWrap.scrollWidth + Math.max(0, actionsWidth - 112);
  return Math.max(startWidth, Math.floor(startWidth + Math.max(0, room) - 1));
}

function initColumnResize() {
  for (const [key, width] of Object.entries(readStoredColumnWidths())) {
    tableWrap.style.setProperty(columnVar(key), `${width}px`);
  }

  document.querySelectorAll<HTMLElement>(".col-resizer").forEach((handle) => {
    const key = handle.dataset.col ?? "";
    const def = COLUMN_DEFS.find((d) => d.key === key);
    const cell = handle.closest<HTMLElement>(".cell");
    if (!def || !cell) return;

    let startX = 0;
    let startWidth = 0;
    let maxWidth = Number.POSITIVE_INFINITY;

    const onMove = (event: PointerEvent) => {
      const desired = Math.round(startWidth + event.clientX - startX);
      const width = Math.min(maxWidth, Math.max(def.min, desired));
      tableWrap.style.setProperty(columnVar(key), `${width}px`);
    };
    const onEnd = (event: PointerEvent) => {
      handle.classList.remove("dragging");
      tableWrap.classList.remove("resizing");
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onEnd);
      handle.removeEventListener("pointercancel", onEnd);
      if (handle.hasPointerCapture(event.pointerId)) {
        handle.releasePointerCapture(event.pointerId);
      }
      persistColumnWidths();
    };

    handle.addEventListener("pointerdown", (event) => {
      if (event.button !== 0) return;
      event.preventDefault();
      startX = event.clientX;
      startWidth = cell.getBoundingClientRect().width;
      maxWidth = maxColumnWidth(startWidth);
      handle.classList.add("dragging");
      tableWrap.classList.add("resizing");
      try {
        handle.setPointerCapture(event.pointerId);
      } catch {
        /* alguns ambientes não suportam captura de ponteiro */
      }
      handle.addEventListener("pointermove", onMove);
      handle.addEventListener("pointerup", onEnd);
      handle.addEventListener("pointercancel", onEnd);
    });

    handle.addEventListener("dblclick", () => {
      tableWrap.style.removeProperty(columnVar(key));
      persistColumnWidths();
    });
  });
}

// ---------------------------------------------------------------------------
// Inicialização
// ---------------------------------------------------------------------------

window.addEventListener("DOMContentLoaded", () => {
  document.querySelector<HTMLAnchorElement>("#x-link")?.addEventListener("click", (event) => {
    event.preventDefault();
    void openUrl("https://x.com/vinisn93").catch((e) => toast(String(e), "err"));
  });

  $("#btn-refresh").addEventListener("click", (e) =>
    void refreshAll((e.currentTarget as HTMLElement).querySelector(".icon") as HTMLElement),
  );
  $("#btn-add").addEventListener("click", () => void doAddAccount());
  $("#btn-save").addEventListener("click", () => void doSaveCurrent());
  $("#btn-settings").addEventListener("click", () => void doSettings());

  initColumnResize();

  void listen<AccountRow[]>("usage-updated", (event) => {
    renderRows(event.payload);
    lastRefreshEl.textContent = `cotas lidas ${new Date().toLocaleTimeString("pt-BR", { hour: "2-digit", minute: "2-digit" })}`;
  });

  void (async () => {
    try {
      const live = await invoke<IdentityView | null>("live_identity");
      if (live?.email) {
        liveTag.hidden = false;
        liveTag.textContent = `ativo: ${live.email}`;
      }
    } catch {
      /* sem login vivo legível */
    }
    await reload();
    void refreshAll();
    await updateCodexStatus();
    window.setInterval(() => void updateCodexStatus(), 30000);
    window.setInterval(() => {
      // mantém os tempos relativos atualizados
      if (currentRows.length) renderRows(currentRows);
    }, 60000);
  })();
});
