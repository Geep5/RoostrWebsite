// Loads the Roostr module (wasm-bindgen `--target web` output beside this
// file) and starts it under this page's base path ("" at the site root).
import init, { run } from "./roostr_web.js";

// gpui's web platform reads keys through a 1x1 <input> it appends to the body
// and focuses on every pointerdown. On a touch-only device that raises the
// on-screen keyboard over the canvas on every tap: keep it closed there.
const touchOnly = window.matchMedia?.("(hover: none) and (pointer: coarse)").matches ?? false;
if (touchOnly) {
  new MutationObserver((records) => {
    for (const record of records) {
      for (const node of record.addedNodes) {
        if (node.tagName === "INPUT") {
          node.readOnly = true;
          node.setAttribute("inputmode", "none");
        }
      }
    }
  }).observe(document.body, { childList: true });
}

const loading = document.getElementById("loading");
const base = new URL(".", import.meta.url).pathname.replace(/\/$/, "");

try {
  const started = performance.now();
  await init();
  run(base);
  // The first frame needs graphics, which start asynchronously after `run`.
  requestAnimationFrame(() => {
    loading?.remove();
    console.info(`Roostr started in ${Math.round(performance.now() - started)} ms`);
  });
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
