// Loads the Roostr module (wasm-bindgen `--target web` output beside this
// file) and starts it under this page's base path ("" at the site root).
import init, { run } from "./roostr_web.js";

const loading = document.getElementById("loading");
const base = new URL(".", import.meta.url).pathname.replace(/\/$/, "");

try {
  const started = performance.now();
  // Rune's canvas announces its first frame on screen.
  document.addEventListener(
    "runepresent",
    () => {
      loading?.remove();
      console.info(`Roostr started in ${Math.round(performance.now() - started)} ms`);
    },
    { once: true },
  );
  await init();
  run(base);
} catch (error) {
  console.error("Roostr failed to start:", error);
  if (loading) {
    loading.innerHTML = "";
    const box = document.createElement("div");
    box.className = "error";
    const title = document.createElement("h2");
    title.textContent = "Roostr could not start in this browser";
    const detail = document.createElement("p");
    detail.textContent = String(error?.message ?? error);
    box.append(title, detail);
    loading.append(box);
  }
}
