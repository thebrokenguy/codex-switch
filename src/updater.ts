import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

// Atualização automática via GitHub Releases: checa em silêncio, baixa,
// valida a assinatura e instala. Em Windows o app encerra antes de instalar
// (limitação dos instaladores), então o relaunch cobre os demais sistemas.

const FIRST_CHECK_DELAY_MS = 10_000;
const RECHECK_INTERVAL_MS = 6 * 60 * 60 * 1000; // 6h

export function startUpdateChecks(): void {
  // Modo dev nunca atualiza: o binário em desenvolvimento não é o instalado.
  if (import.meta.env.DEV) {
    return;
  }
  window.setTimeout(() => void runUpdateCheck(), FIRST_CHECK_DELAY_MS);
  window.setInterval(() => void runUpdateCheck(), RECHECK_INTERVAL_MS);
}

async function runUpdateCheck(): Promise<void> {
  try {
    const update = await check();
    if (!update) {
      return;
    }
    console.log(`atualização disponível: ${update.version}`);
    await update.downloadAndInstall();
    await relaunch();
  } catch (e) {
    // Falha de rede ou de atualização nunca derruba o app.
    console.warn("falha verificando atualização:", e);
  }
}
