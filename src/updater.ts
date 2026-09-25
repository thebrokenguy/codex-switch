import { invoke } from "@tauri-apps/api/core";

// Auto-update do app portátil: o Rust checa a release no GitHub, baixa o exe
// novo e confere o SHA-256. A troca do arquivo acontece na próxima abertura
// (o app reabre sozinho já na versão nova).

interface UpdateStatus {
  status: "skipped" | "up_to_date" | "staged" | "error";
  version: string | null;
  message: string | null;
}

const FIRST_CHECK_DELAY_MS = 10_000;
const RECHECK_INTERVAL_MS = 6 * 60 * 60 * 1000; // 6h

export function startUpdateChecks(notify: (message: string) => void): void {
  // Modo dev nunca atualiza: o binário em desenvolvimento não é o distribuído.
  if (import.meta.env.DEV) {
    return;
  }
  window.setTimeout(() => void runUpdateCheck(notify), FIRST_CHECK_DELAY_MS);
  window.setInterval(() => void runUpdateCheck(notify), RECHECK_INTERVAL_MS);
}

async function runUpdateCheck(notify: (message: string) => void): Promise<void> {
  try {
    const result = await invoke<UpdateStatus>("check_for_update");
    if (result.status === "staged" && result.version) {
      notify(
        `Atualização ${result.version} baixada e verificada. Ela vale na próxima abertura do app.`,
      );
    }
    if (result.status === "error") {
      console.warn("falha verificando atualização:", result.message);
    }
  } catch (e) {
    // Falha de rede nunca derruba o app.
    console.warn("falha verificando atualização:", e);
  }
}
