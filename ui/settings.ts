import { getVersion } from "@tauri-apps/api/app";

// Placeholder until M7 builds the settings page.
getVersion().then((v) => {
  document.getElementById("version")!.textContent = `版本 ${v}`;
});
