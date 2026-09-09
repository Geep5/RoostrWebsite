/** Explicit, origin-bound pairing; localhost is never treated as trusted. */
export interface PairedSession { token: string; expiresAt: number; role: "ui" }
const SESSION_KEY = "roostr-local-pairing";
const listeners = new Set<() => void>();
let expiryTimer: ReturnType<typeof setTimeout> | undefined;
export const LOCAL_API = "http://127.0.0.1:7333";
const HARNESS_API = "http://127.0.0.1:7334";
export class PairingError extends Error {
	constructor(message = "Pair this browser with your local daemon first.") { super(message); this.name = "PairingError"; }
}
function notify(): void { for (const callback of listeners) callback(); }
function scheduleExpiry(expiresAt: number): void {
	clearTimeout(expiryTimer);
	expiryTimer = setTimeout(() => unpairLocal(), Math.max(0, expiresAt - Date.now()));
}
export function pairedSession(): PairedSession | null {
	if (typeof sessionStorage === "undefined") return null;
	try {
		const value = JSON.parse(sessionStorage.getItem(SESSION_KEY) ?? "null") as PairedSession | null;
		if (!value || !/^[a-f0-9]{64}$/i.test(value.token) || value.role !== "ui" || !Number.isFinite(value.expiresAt) || value.expiresAt <= Date.now()) return null;
		return value;
	} catch { return null; }
}
export function onPairingChange(callback: () => void): () => void {
	listeners.add(callback);
	const session = pairedSession();
	if (session) scheduleExpiry(session.expiresAt);
	return () => listeners.delete(callback);
}
export function unpairLocal(): void {
	clearTimeout(expiryTimer);
	if (typeof sessionStorage !== "undefined") sessionStorage.removeItem(SESSION_KEY);
	notify();
}
export async function pairLocal(code: string): Promise<void> {
	if (!code.trim()) throw new PairingError("Enter the one-use code printed in your daemon terminal.");
	const response = await fetch(`${LOCAL_API}/api/pair`, {
		method: "POST", headers: { "Content-Type": "application/json" },
		body: JSON.stringify({ code: code.trim() }), credentials: "omit", redirect: "error",
	});
	if (!response.ok) throw new PairingError("Pairing failed. The code may have expired or already been used; obtain a new code from the daemon terminal.");
	const value = await response.json();
	const expiresAt = value.expiresAt;
	if (!/^[a-f0-9]{64}$/i.test(value.token ?? "") || value.role !== "ui" || !Number.isFinite(expiresAt) || expiresAt <= Date.now()) throw new PairingError("The daemon returned an invalid pairing session.");
	sessionStorage.setItem(SESSION_KEY, JSON.stringify({ token: value.token, expiresAt, role: "ui" }));
	scheduleExpiry(expiresAt);
	notify();
}
function endpoint(base: string, path: string): string {
	if (!path.startsWith("/") || path.startsWith("//") || path.includes("\\")) throw new Error("Local API paths must be relative to the paired daemon.");
	const url = new URL(path, base);
	if (url.origin !== base) throw new Error("Invalid local API origin.");
	return url.href;
}
async function pairedFetch(base: string, path: string, init: RequestInit = {}): Promise<Response> {
	const url = endpoint(base, path);
	const session = pairedSession();
	if (!session) throw new PairingError("Pairing is missing or expired. Enter a fresh terminal code to reconnect.");
	const headers = new Headers(init.headers);
	headers.set("Authorization", `Bearer ${session.token}`);
	const response = await fetch(url, { ...init, headers, credentials: "omit", redirect: "error" });
	if (pairedSession()?.token !== session.token) {
		await response.body?.cancel();
		throw new PairingError("The pairing changed while this request was in flight. Reconnect and try again.");
	}
	if (response.status === 401) {
		unpairLocal();
		throw new PairingError("The daemon rejected this pairing. Enter a fresh terminal code to reconnect.");
	}
	return response;
}
export const localFetch = (path: string, init?: RequestInit): Promise<Response> => pairedFetch(LOCAL_API, path, init);
export const harnessFetch = (path: string, init?: RequestInit): Promise<Response> => pairedFetch(HARNESS_API, path, init);
export async function localJSON<T>(path: string, init?: RequestInit): Promise<T> {
	const response = await localFetch(path, init);
	if (!response.ok) throw new Error((await response.text()) || `Local daemon request failed (${response.status})`);
	return response.json() as Promise<T>;
}
/** Streaming fetch permits Authorization headers, unlike EventSource. */
export async function streamLocalEvents(path: string, signal: AbortSignal, onEvent: (event: string, data: string) => void): Promise<void> {
	const response = await localFetch(path, { signal, headers: { Accept: "text/event-stream" } });
	if (!response.ok || !response.body) throw new Error(`Local event stream unavailable (${response.status})`);
	const reader = response.body.getReader();
	const decoder = new TextDecoder();
	let buffer = "", event = "message", data: string[] = [];
	try {
		for (;;) {
			const chunk = await reader.read();
			if (chunk.done) break;
			buffer += decoder.decode(chunk.value, { stream: true });
			let newline: number;
			while ((newline = buffer.indexOf("\n")) >= 0) {
				const line = buffer.slice(0, newline).replace(/\r$/, "");
				buffer = buffer.slice(newline + 1);
				if (!line) { if (data.length) onEvent(event, data.join("\n")); event = "message"; data = []; }
				else if (line.startsWith("event:")) event = line.slice(6).replace(/^ /, "");
				else if (line.startsWith("data:")) data.push(line.slice(5).replace(/^ /, ""));
			}
		}
	} finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
	if (!signal.aborted) throw new Error("Local event stream disconnected.");
}
