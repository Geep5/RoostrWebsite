/**
 * Capability requests, staged and resolved from object pages. The DAG
 * carries only the intent - a mailbox message on the installation object;
 * execution waits for a paired human's approval on the owning machine.
 *
 * Shared by the capability page (choose machine, request setup) and the
 * installation page (act on one row, approve what waits). The retired
 * machine modal's copies lived in Machine.svelte.
 */

import { fetchAllQuery, mailbox, note } from "$lib/api";
import { harnessFetch } from "$lib/local-transport";
import { fieldStr } from "$lib/types";

export interface CapabilityRequest {
	objectId: string;
	messageId: string;
	key: string;
	account: string;
	operation: string;
	sender: { objectId: string; agentId: string };
	status: string;
	error: string;
	canApprove: boolean;
	fields?: Array<{ key: string; label: string; secret: boolean }>;
}

/** Requests waiting on a human or in flight, harness-side view. */
export async function listCapabilityRequests(): Promise<CapabilityRequest[]> {
	const res = await harnessFetch("/capability-requests");
	if (!res.ok) throw new Error(`Cannot load approvals (HTTP ${res.status}).`);
	return ((await res.json()) as { requests: CapabilityRequest[] }).requests;
}

/** This machine's stable id ("" when the paired harness is unreachable). */
export async function thisMachineId(): Promise<string> {
	try {
		const res = await harnessFetch("/machine");
		if (!res.ok) return "";
		return ((await res.json()) as { id: string }).id;
	} catch {
		return "";
	}
}

/**
 * Stage one operation into the owning installation's mailbox. Throws with a
 * human-readable reason. Dedupes: the same open request is not sent twice.
 * Works from any device that can write the vault - delivery reaches the
 * owning machine through sync.
 */
export async function stageCapabilityRequest(key: string, operation: string, account = ""): Promise<void> {
	const id = await thisMachineId();
	if (!id) throw new Error("Cannot identify the owning machine. Is the harness running?");
	const [installRows, machineRows] = await Promise.all([fetchAllQuery({ type: "install" }), fetchAllQuery({ type: "machine" })]);
	let installationId = installRows.find((row) => fieldStr(row.fields, "key") === key && fieldStr(row.fields, "machine_id") === id && fieldStr(row.fields, "account") === account)?.id;
	if (!installationId && key === "google" && account) {
		installationId = (await note.create(`Google ${account}`, "install", { key: { stringValue: key }, machine_id: { stringValue: id }, account: { stringValue: account }, status: { stringValue: "missing" } })).id;
	}
	if (!installationId) throw new Error("This machine has not published the installation object yet.");
	const source = machineRows.find((row) => fieldStr(row.fields, "machine_id") === id);
	if (!source) throw new Error("This machine has not published its object yet.");
	await sendCapabilityRequest(source.id, installationId, key, operation, account);
}

/**
 * Stage onto a specific installation object, sender already known - the
 * /setup checklist's shape, where the target machine may not be this one.
 * Dedupes against open requests for the same operation.
 */
export async function sendCapabilityRequest(senderObjectId: string, installationId: string, key: string, operation: string, account = ""): Promise<void> {
	const open = await listCapabilityRequests().catch(() => [] as CapabilityRequest[]);
	if (open.some((r) => r.objectId === installationId && r.operation === operation && ["pending", "awaiting_approval", "processing"].includes(r.status))) {
		throw new Error("This request is already waiting for approval.");
	}
	await mailbox.send({ id: crypto.randomUUID(), exchangeId: crypto.randomUUID(), sender: { objectId: senderObjectId, agentId: "" }, recipients: [{ objectId: installationId, agentId: "" }], text: `Request ${operation} for ${key}${account ? ` (${account})` : ""}.`, replyTo: "", sentAt: Date.now(), title: "Capability request", requestReply: true, historical: false, operation, author: "" });
}

/** Approve, reject, or confirm a login. Approval executes on this machine's harness. */
export async function resolveCapabilityRequest(request: CapabilityRequest, action: "approve" | "reject" | "finish-login", fields?: Record<string, string>): Promise<{ active?: boolean }> {
	const body: { objectId: string; messageId: string; fields?: Record<string, string> } = { objectId: request.objectId, messageId: request.messageId };
	if (action === "approve" && request.operation === "auth.save") body.fields = fields ?? {};
	const res = await harnessFetch(`/capability-requests/${action}`, { method: "POST", body: JSON.stringify(body) });
	const result = (await res.json().catch(() => ({}))) as { error?: string; active?: boolean };
	if (!res.ok || result.error) throw new Error(result.error ?? `HTTP ${res.status}`);
	return result;
}
